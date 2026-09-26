/*
 * aeros-agent: the guest half of AerOS "seamless" Linux windows.
 *
 * Runs inside the X session. It publishes the list of top-level client
 * windows (position, size, title, focus) into a mailbox at the tail of the
 * shared framebuffer region (reachable by mmap'ing /dev/fb0), where the host
 * hypervisor reads it, and executes the commands the host leaves in a second
 * mailbox: close / focus a window, launch a program.
 *
 * It also keeps windows from overlapping each other on the guest screen (the
 * host shows each window as a crop of that one screen, so overlap would leak
 * a neighbour into the picture) by tiling new windows into free space.
 *
 * Clipboard mailboxes live at 0xA10000 (host->guest) and 0xA20000 (guest->host).
 *
 * Mailbox layout (all little-endian u32 unless noted) - keep in sync with
 * kernel/src/svm.rs:
 *   TABLE @ 0xA00000: magic 'AEWT', seq (odd while writing), count,
 *                     screen_w, screen_h, 3 pad, then 16 x window {
 *                     id, x, y, w, h, flags(1=viewable,2=focused), title[64] }
 *   CMD   @ 0xA01000: magic 'AEWC', seq_host, seq_ack, pad, then a ring of 16 x
 *                     {op, a0..a3, text[128]}; slot = counter % 16
 *                     ops: 1 close(id) 2 focus(id) 3 spawn(text) 4 move(id,x,y) 5 resize(id,w,h)
 */
#include <ctype.h>
#include <dirent.h>
#include <errno.h>
#include <sys/stat.h>
#include <sys/wait.h>
#include <time.h>
#include <X11/Xatom.h>
#include <X11/Xlib.h>
#include <X11/extensions/Xfixes.h>
#include <fcntl.h>
#include <signal.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <unistd.h>

#define FB_BYTES 0xC00000u
#define TABLE_OFF 0xA00000u
#define CMD_OFF 0xA01000u
#define MAGIC_TABLE 0x54574541u /* "AEWT" */
#define MAGIC_CMD 0x43574541u   /* "AEWC" */
#define MAX_WIN 16
#define CLIP_IN_OFF 0xA10000u  /* host -> guest clipboard */
#define CLIP_OUT_OFF 0xA20000u /* guest -> host clipboard */
#define CLIP_MAX 60000u
#define MAGIC_CLIP_IN 0x49434541u  /* "AECI" */
#define MAGIC_CLIP_OUT 0x4f434541u /* "AECO" */
#define CMD_SLOTS 16
#define WEB_REQ_OFF 0xA30000u /* host -> guest: page to fetch */
#define WEB_RES_OFF 0xA40000u /* guest -> host: the page as text */
#define APPS_OFF 0xA50000u    /* guest -> host: installed applications */
#define WEB_MAX 64000u
#define MAX_APPS 96
#define MAGIC_WEB_REQ 0x51574541u /* "AEWQ" */
#define MAGIC_WEB_RES 0x53574541u /* "AEWS" */
#define MAGIC_APPS 0x50414541u    /* "AEAP" */

struct gwin {
    uint32_t id;
    int32_t x, y;
    uint32_t w, h;
    uint32_t flags;
    char title[64];
};

struct table {
    uint32_t magic, seq, count, screen_w, screen_h, pad[3];
    struct gwin win[MAX_WIN];
};

/* One queued command. The mailbox is a ring of CMD_SLOTS of these, so a burst
 * of launches/closes from the host is not lost to a single-slot overwrite. */
struct cmd {
    uint32_t op, a0, a1, a2, a3;
    char text[128];
};

struct cmdring {
    uint32_t magic, seq_host, seq_ack, pad;
    struct cmd slot[CMD_SLOTS];
};

/* Clipboard mailboxes. IN: the host writes len+data, then bumps seq_host; the
 * agent copies it and sets seq_ack. OUT: seqlock (odd while writing). */
struct clip_in {
    uint32_t magic, seq_host, seq_ack, len;
    char data[CLIP_MAX];
};

struct clip_out {
    uint32_t magic, seq, len, pad;
    char data[CLIP_MAX];
};

struct web_req {
    uint32_t magic, seq_host, seq_ack, cols;
    char url[512];
};

struct web_res {
    uint32_t magic, seq;
    uint32_t status; /* 0 done, 1 failed, 2 fetching */
    uint32_t len;
    char data[WEB_MAX];
};

struct app_ent {
    char name[48];
    char exec[112];
    char cat[24];
};

struct apps_tab {
    uint32_t magic, seq, count;
    uint32_t install_state; /* 0 idle, 1 installing, 2 done, 3 failed */
    struct app_ent app[MAX_APPS];
};

static Display *dpy;
static Window root;
static Atom a_client_list, a_active, a_name, a_utf8, a_close, a_frame;

static void client_message(Window target, Window w, Atom type, long l0, long l1, long l2) {
    XEvent ev;
    memset(&ev, 0, sizeof ev);
    ev.xclient.type = ClientMessage;
    ev.xclient.window = w;
    ev.xclient.message_type = type;
    ev.xclient.format = 32;
    ev.xclient.data.l[0] = l0;
    ev.xclient.data.l[1] = l1;
    ev.xclient.data.l[2] = l2;
    XSendEvent(dpy, target, False, SubstructureRedirectMask | SubstructureNotifyMask, &ev);
}

static void read_title(Window w, char *out, size_t cap) {
    Atom type;
    int fmt;
    unsigned long n, after;
    unsigned char *data = NULL;
    out[0] = 0;
    if (XGetWindowProperty(dpy, w, a_name, 0, 32, False, a_utf8, &type, &fmt, &n, &after, &data) ==
            Success &&
        data && n > 0) {
        snprintf(out, cap, "%s", (char *)data);
    } else {
        char *name = NULL;
        if (XFetchName(dpy, w, &name) && name) {
            snprintf(out, cap, "%s", name);
            XFree(name);
        }
    }
    if (data) XFree(data);
}

static int overlaps(const struct gwin *a, int x, int y, unsigned w, unsigned h) {
    return x < a->x + (int)a->w && a->x < x + (int)w && y < a->y + (int)a->h && a->y < y + (int)h;
}

/* First free spot (row-major, 8 px grid) for a w x h window among `others`;
 * returns 0 (and (0,0)) when there is none. */
static int find_slot(const struct gwin *others, int n, unsigned w, unsigned h, int sw, int sh,
                     int *ox, int *oy) {
    for (int y = 0; y + (int)h <= sh; y += 8) {
        for (int x = 0; x + (int)w <= sw; x += 8) {
            int free_spot = 1;
            for (int i = 0; i < n && free_spot; i++)
                if (overlaps(&others[i], x, y, w, h)) free_spot = 0;
            if (free_spot) {
                *ox = x;
                *oy = y;
                return 1;
            }
        }
    }
    *ox = 0;
    *oy = 0;
    return 0;
}

/* No free spot left for another window: lay every window out in an even grid
 * (cols x rows cells covering the screen) so none overlaps another. Windows
 * that refuse to get that small (minimum size hints) may still overlap. */
static void retile_all(struct gwin *wins, int n, int sw, int sh) {
    int cols = 1;
    while (cols * cols < n) cols++;
    int rows = (n + cols - 1) / cols;
    unsigned cw = (unsigned)(sw / cols), ch = (unsigned)(sh / rows);
    for (int k = 0; k < n; k++) {
        int x = (k % cols) * (int)cw, y = (k / cols) * (int)ch;
        XMoveResizeWindow(dpy, (Window)wins[k].id, x, y, cw, ch);
        wins[k].x = x;
        wins[k].y = y;
        wins[k].w = cw;
        wins[k].h = ch;
    }
}

static int window_gone(Window w) {
    XWindowAttributes at;
    return !XGetWindowAttributes(dpy, w, &at);
}

static void run_command(struct cmd *c) {
    Window w = (Window)c->a0;
    switch (c->op) {
    case 1: /* close */
        client_message(root, w, a_close, 0, 2, 0);
        break;
    case 2: /* focus + raise */
        client_message(root, w, a_active, 2, 0, 0);
        XRaiseWindow(dpy, w);
        break;
    case 3: /* spawn */
        c->text[sizeof c->text - 1] = 0;
        if (fork() == 0) {
            setsid();
            /* Keep the program's output for debugging (no terminal here). */
            int log = open("/tmp/aeros-spawn.log", O_WRONLY | O_CREAT | O_APPEND, 0644);
            if (log >= 0) {
                dup2(log, 1);
                dup2(log, 2);
            }
            execl("/bin/sh", "sh", "-c", c->text, (char *)NULL);
            _exit(127);
        }
        break;
    case 4: /* move */
        if (!window_gone(w)) XMoveWindow(dpy, w, (int)c->a1, (int)c->a2);
        break;
    case 5: /* resize (a1 = width, a2 = height) */
        if (!window_gone(w)) {
            unsigned rw = c->a1 < 120 ? 120 : c->a1, rh = c->a2 < 80 ? 80 : c->a2;
            XResizeWindow(dpy, w, rw, rh);
        }
        break;
    }
}

/* ---- Clipboard bridge -------------------------------------------------
 * Host -> guest: the text the host leaves in the IN mailbox is made the X
 * CLIPBOARD (and PRIMARY) selection, owned by a hidden window that answers
 * SelectionRequests. Guest -> host: XFixes tells us when another client takes
 * the CLIPBOARD; we fetch it as text and publish it in the OUT mailbox. */
static Window cwin;
static Atom a_clipboard, a_targets, a_clip_prop;
static int xfixes_base;
static char own_text[CLIP_MAX + 1];
static size_t own_len;
static char last_pub[CLIP_MAX + 1];
static size_t last_pub_len;

static void clipboard_init(void) {
    int err;
    cwin = XCreateSimpleWindow(dpy, root, 0, 0, 1, 1, 0, 0, 0);
    a_clipboard = XInternAtom(dpy, "CLIPBOARD", False);
    a_targets = XInternAtom(dpy, "TARGETS", False);
    a_clip_prop = XInternAtom(dpy, "AEROS_CLIP", False);
    if (XFixesQueryExtension(dpy, &xfixes_base, &err))
        XFixesSelectSelectionInput(dpy, root, a_clipboard, XFixesSetSelectionOwnerNotifyMask);
}

static void serve_selection(XSelectionRequestEvent *rq) {
    XSelectionEvent reply;
    memset(&reply, 0, sizeof reply);
    reply.type = SelectionNotify;
    reply.display = rq->display;
    reply.requestor = rq->requestor;
    reply.selection = rq->selection;
    reply.target = rq->target;
    reply.property = rq->property ? rq->property : rq->target;
    reply.time = rq->time;
    if (rq->target == a_targets) {
        Atom targets[3] = {a_targets, a_utf8, XA_STRING};
        XChangeProperty(dpy, rq->requestor, reply.property, XA_ATOM, 32, PropModeReplace,
                        (unsigned char *)targets, 3);
    } else if (rq->target == a_utf8 || rq->target == XA_STRING) {
        XChangeProperty(dpy, rq->requestor, reply.property, rq->target, 8, PropModeReplace,
                        (unsigned char *)own_text, (int)own_len);
    } else {
        reply.property = None;
    }
    XSendEvent(dpy, rq->requestor, False, 0, (XEvent *)&reply);
}

static void clipboard_poll(volatile struct clip_in *in, volatile struct clip_out *out) {
    if (in->seq_host != in->seq_ack) {
        uint32_t len = in->len > CLIP_MAX ? CLIP_MAX : in->len;
        memcpy(own_text, (const void *)in->data, len);
        own_len = len;
        memcpy(last_pub, own_text, len); /* don't echo it back to the host */
        last_pub_len = len;
        in->seq_ack = in->seq_host;
        XSetSelectionOwner(dpy, a_clipboard, cwin, CurrentTime);
        XSetSelectionOwner(dpy, XA_PRIMARY, cwin, CurrentTime);
        XFlush(dpy);
    }
    while (XPending(dpy)) {
        XEvent ev;
        XNextEvent(dpy, &ev);
        if (ev.type == SelectionRequest) {
            serve_selection(&ev.xselectionrequest);
        } else if (ev.type == xfixes_base + XFixesSelectionNotify) {
            XFixesSelectionNotifyEvent *sn = (XFixesSelectionNotifyEvent *)&ev;
            if (sn->selection == a_clipboard && sn->owner != cwin && sn->owner != None)
                XConvertSelection(dpy, a_clipboard, a_utf8, a_clip_prop, cwin, CurrentTime);
        } else if (ev.type == SelectionNotify && ev.xselection.property == a_clip_prop) {
            Atom type;
            int fmt;
            unsigned long n = 0, after;
            unsigned char *data = NULL;
            if (XGetWindowProperty(dpy, cwin, a_clip_prop, 0, CLIP_MAX / 4, True, AnyPropertyType,
                                   &type, &fmt, &n, &after, &data) == Success && data) {
                size_t len = n > CLIP_MAX ? CLIP_MAX : n;
                if (fmt == 8 && len > 0 && (len != last_pub_len || memcmp(data, last_pub, len))) {
                    memcpy(last_pub, data, len);
                    last_pub_len = len;
                    out->seq++;
                    __sync_synchronize();
                    memcpy((void *)out->data, data, len);
                    out->len = (uint32_t)len;
                    __sync_synchronize();
                    out->seq++;
                }
                XFree(data);
            }
        }
    }
}

/* ---- Web fetch ----------------------------------------------------------
 * The host's browser asks for a page by URL; w3m (with real TLS) fetches it and
 * renders it as text, which is published back for the host to draw. The
 * fetch runs as a child process and is polled, so the window table keeps
 * updating meanwhile. */
static int web_fd = -1;
static pid_t web_pid;
static size_t web_len;
static time_t web_started;
static char web_buf[WEB_MAX];

static void web_publish(volatile struct web_res *res, uint32_t status, const char *data, size_t len) {
    if (len > WEB_MAX) len = WEB_MAX;
    res->seq++;
    __sync_synchronize();
    if (len) memcpy((void *)res->data, data, len);
    res->status = status;
    res->len = (uint32_t)len;
    __sync_synchronize();
    res->seq++;
}

static void web_finish(volatile struct web_res *res, int failed, const char *note) {
    if (web_fd >= 0) close(web_fd);
    web_fd = -1;
    if (web_pid > 0) {
        kill(web_pid, SIGKILL);
        waitpid(web_pid, NULL, 0);
    }
    web_pid = 0;
    if (web_len == 0 && note) {
        snprintf(web_buf, sizeof web_buf, "%s", note);
        web_len = strlen(web_buf);
        failed = 1;
    }
    web_publish(res, failed ? 1 : 0, web_buf, web_len);
}

static void web_poll(volatile struct web_req *req, volatile struct web_res *res) {
    if (req->seq_host != req->seq_ack) {
        char url[512];
        uint32_t cols = req->cols < 40 ? 40 : req->cols > 200 ? 200 : req->cols;
        memcpy(url, (const void *)req->url, sizeof url);
        url[sizeof url - 1] = 0;
        req->seq_ack = req->seq_host;
        if (web_fd >= 0) web_finish(res, 1, NULL); /* superseded */
        int fds[2];
        if (pipe(fds) == 0) {
            pid_t pid = fork();
            if (pid == 0) {
                dup2(fds[1], 1);
                dup2(fds[1], 2);
                close(fds[0]);
                close(fds[1]);
                char colarg[16];
                snprintf(colarg, sizeof colarg, "%u", cols);
                setenv("HOME", "/tmp/home", 1);
                execl("/usr/bin/w3m", "w3m", "-dump", "-cols", colarg, "-o", "display_link_number=1", "-o", "user_agent=Mozilla/5.0 (X11; Linux x86_64) AerOS",
                      url, (char *)NULL);
                printf("This system has no web fetcher (w3m) installed.\n");
                _exit(127);
            }
            close(fds[1]);
            if (pid > 0) {
                fcntl(fds[0], F_SETFL, O_NONBLOCK);
                web_fd = fds[0];
                web_pid = pid;
                web_len = 0;
                web_started = time(NULL);
                web_publish(res, 2, "", 0);
            } else {
                close(fds[0]);
                web_publish(res, 1, "fork failed", 11);
            }
        }
    }
    if (web_fd < 0) return;
    for (int i = 0; i < 8; i++) {
        ssize_t n = read(web_fd, web_buf + web_len, WEB_MAX - web_len);
        if (n > 0) {
            web_len += (size_t)n;
            if (web_len >= WEB_MAX) break;
        } else if (n == 0) {
            web_finish(res, 0, "The page returned nothing.");
            return;
        } else {
            break; /* EAGAIN */
        }
    }
    if (web_len >= WEB_MAX) {
        web_finish(res, 0, NULL);
        return;
    }
    if (time(NULL) - web_started > 45) web_finish(res, 1, "The page took too long to load.");
}

/* ---- Installed applications ---------------------------------------------
 * The .desktop files under /usr/share/applications, as the host's store shows
 * them; rescanned when the directory changes (an install adds files). */
static struct app_ent apps_found[MAX_APPS];
static int apps_count;

static void trim(char *text) {
    size_t n = strlen(text);
    while (n && (text[n - 1] == '\n' || text[n - 1] == '\r' || text[n - 1] == ' ')) text[--n] = 0;
}

static int app_cmp(const void *a, const void *b) {
    return strcasecmp(((const struct app_ent *)a)->name, ((const struct app_ent *)b)->name);
}

static void scan_dir(const char *dirpath) {
    DIR *dir = opendir(dirpath);
    if (!dir) return;
    struct dirent *entry;
    while ((entry = readdir(dir)) && apps_count < MAX_APPS) {
        size_t len = strlen(entry->d_name);
        if (len < 9 || strcmp(entry->d_name + len - 8, ".desktop") != 0) continue;
        char path[300];
        snprintf(path, sizeof path, "%s/%s", dirpath, entry->d_name);
        FILE *file = fopen(path, "r");
        if (!file) continue;
        struct app_ent app;
        memset(&app, 0, sizeof app);
        int in_entry = 0, hidden = 0, is_app = 0, has_name = 0, terminal = 0;
        char line[512];
        while (fgets(line, sizeof line, file)) {
            trim(line);
            if (line[0] == '[') {
                in_entry = strcmp(line, "[Desktop Entry]") == 0;
                continue;
            }
            if (!in_entry) continue;
            if (strncmp(line, "Name=", 5) == 0 && !has_name) {
                snprintf(app.name, sizeof app.name, "%s", line + 5);
                has_name = 1;
            } else if (strncmp(line, "Exec=", 5) == 0 && !app.exec[0]) {
                snprintf(app.exec, sizeof app.exec, "%s", line + 5);
            } else if (strcmp(line, "Type=Application") == 0) {
                is_app = 1;
            } else if (strcmp(line, "Terminal=true") == 0) {
                terminal = 1;
            } else if (strcmp(line, "NoDisplay=true") == 0 || strcmp(line, "Hidden=true") == 0) {
                hidden = 1;
            } else if (strncmp(line, "Categories=", 11) == 0) {
                if (strstr(line + 11, "Settings")) hidden = 1; /* settings dialogs are not apps */
                char *semi = strchr(line + 11, ';');
                if (semi) *semi = 0;
                snprintf(app.cat, sizeof app.cat, "%s", line + 11);
            }
        }
        fclose(file);
        if (hidden || !is_app || !has_name || !app.exec[0]) continue;
        /* Drop the field codes (%U, %f ...) from the command line. */
        for (char *at = app.exec; *at; at++) {
            if (*at == '%' && at[1]) {
                memmove(at, at + 2, strlen(at + 2) + 1);
                at--;
            }
        }
        /* Flatpak launchers wrap the file arguments in @@u ... @@. */
        for (char *at; (at = strstr(app.exec, " @@u")) || (at = strstr(app.exec, " @@"));) {
            memmove(at, at + (at[3] == 'u' ? 4 : 3), strlen(at + (at[3] == 'u' ? 4 : 3)) + 1);
        }
        trim(app.exec);
        if (terminal) {
            /* A terminal program: run it inside an xterm. */
            char wrapped[112];
            snprintf(wrapped, sizeof wrapped, "xterm -e %s", app.exec);
            snprintf(app.exec, sizeof app.exec, "%s", wrapped);
        }
        int duplicate = 0;
        for (int i = 0; i < apps_count; i++)
            if (strcmp(apps_found[i].name, app.name) == 0) duplicate = 1;
        if (!duplicate) apps_found[apps_count++] = app;
    }
    closedir(dir);
}

static void scan_apps(void) {
    apps_count = 0;
    scan_dir("/usr/share/applications");
    scan_dir("/var/lib/flatpak/exports/share/applications");
    qsort(apps_found, (size_t)apps_count, sizeof apps_found[0], app_cmp);
}

static uint32_t install_state(void) {
    FILE *file = fopen("/tmp/aeros-install.log", "r");
    if (!file) return 0;
    char line[256], last[256] = "";
    while (fgets(line, sizeof line, file)) {
        trim(line);
        if (line[0]) snprintf(last, sizeof last, "%s", line);
    }
    fclose(file);
    if (strcmp(last, "DONE") == 0) return 2;
    if (strcmp(last, "FAILED") == 0) return 3;
    return 1;
}

static void apps_poll(volatile struct apps_tab *tab) {
    static time_t last_check;
    static time_t last_mtime;
    static uint32_t last_state = 99;
    time_t now = time(NULL);
    if (now == last_check) return;
    last_check = now;
    struct stat st;
    time_t mtime = 0;
    if (stat("/usr/share/applications", &st) == 0) mtime = st.st_mtime > st.st_ctime ? st.st_mtime : st.st_ctime;
    if (stat("/var/lib/flatpak/exports/share/applications", &st) == 0)
        mtime += st.st_mtime > st.st_ctime ? st.st_mtime : st.st_ctime;
    uint32_t state = install_state();
    if (mtime == last_mtime && state == last_state && tab->seq != 0) return;
    last_mtime = mtime;
    last_state = state;
    scan_apps();
    tab->seq++;
    __sync_synchronize();
    tab->count = (uint32_t)apps_count;
    tab->install_state = state;
    memcpy((void *)tab->app, apps_found, sizeof apps_found[0] * (size_t)apps_count);
    __sync_synchronize();
    tab->seq++;
}

/* Windows vanish between listing and inspecting them; an X error there must
 * not kill the agent (Xlib's default handler exits the process). */
static int ignore_x_error(Display *d, XErrorEvent *e) {
    (void)d;
    (void)e;
    return 0;
}

int main(void) {
    signal(SIGCHLD, SIG_IGN); /* spawned programs are reaped automatically */
    XSetErrorHandler(ignore_x_error);
    for (int tries = 0; tries < 120 && !(dpy = XOpenDisplay(NULL)); tries++) sleep(1);
    if (!dpy) return 1;
    root = DefaultRootWindow(dpy);
    a_client_list = XInternAtom(dpy, "_NET_CLIENT_LIST", False);
    a_active = XInternAtom(dpy, "_NET_ACTIVE_WINDOW", False);
    a_name = XInternAtom(dpy, "_NET_WM_NAME", False);
    a_utf8 = XInternAtom(dpy, "UTF8_STRING", False);
    a_close = XInternAtom(dpy, "_NET_CLOSE_WINDOW", False);
    a_frame = XInternAtom(dpy, "_NET_FRAME_EXTENTS", False);
    (void)a_frame;

    int fd = open("/dev/fb0", O_RDWR);
    if (fd < 0) return 2;
    uint8_t *map = mmap(NULL, FB_BYTES, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
    if (map == MAP_FAILED) return 3;
    volatile struct table *tab = (volatile struct table *)(map + TABLE_OFF);
    volatile struct cmdring *ring = (volatile struct cmdring *)(map + CMD_OFF);
    volatile struct clip_in *clip_in = (volatile struct clip_in *)(map + CLIP_IN_OFF);
    volatile struct clip_out *clip_out = (volatile struct clip_out *)(map + CLIP_OUT_OFF);
    tab->magic = MAGIC_TABLE;
    tab->seq = 0;
    ring->magic = MAGIC_CMD;
    ring->seq_ack = ring->seq_host;
    clip_in->magic = MAGIC_CLIP_IN;
    clip_in->seq_ack = clip_in->seq_host;
    clip_out->magic = MAGIC_CLIP_OUT;
    clip_out->seq = 0;
    clipboard_init();
    volatile struct web_req *web_req = (volatile struct web_req *)(map + WEB_REQ_OFF);
    volatile struct web_res *web_res = (volatile struct web_res *)(map + WEB_RES_OFF);
    volatile struct apps_tab *apps_tab = (volatile struct apps_tab *)(map + APPS_OFF);
    web_req->magic = MAGIC_WEB_REQ;
    web_req->seq_ack = web_req->seq_host;
    web_res->magic = MAGIC_WEB_RES;
    web_res->seq = 0;
    apps_tab->magic = MAGIC_APPS;
    apps_tab->seq = 0;

    XWindowAttributes rootat;
    XGetWindowAttributes(dpy, root, &rootat);
    int sw = rootat.width, sh = rootat.height;

    struct gwin placed[MAX_WIN];
    int nplaced = 0;
    struct gwin last[MAX_WIN];
    int last_count = -1;

    for (;;) {
        /* Drain the command ring; a host that got ahead by more than the ring
         * holds loses the oldest commands, never the newest. */
        uint32_t pending = ring->seq_host - ring->seq_ack;
        if (pending > CMD_SLOTS) ring->seq_ack = ring->seq_host - CMD_SLOTS;
        while (ring->seq_ack != ring->seq_host) {
            struct cmd local;
            memcpy(&local, (const void *)&ring->slot[ring->seq_ack % CMD_SLOTS], sizeof local);
            run_command(&local);
            ring->seq_ack++;
        }
        clipboard_poll(clip_in, clip_out);
        web_poll(web_req, web_res);
        apps_poll(apps_tab);

        Atom type;
        int fmt;
        unsigned long n = 0, after;
        unsigned char *data = NULL;
        Window active = 0;
        if (XGetWindowProperty(dpy, root, a_active, 0, 1, False, XA_WINDOW, &type, &fmt, &n, &after,
                               &data) == Success && data && n == 1)
            active = *(Window *)data;
        if (data) XFree(data);
        data = NULL;

        struct gwin now[MAX_WIN];
        int count = 0;
        if (XGetWindowProperty(dpy, root, a_client_list, 0, 64, False, XA_WINDOW, &type, &fmt, &n,
                               &after, &data) == Success && data) {
            Window *list = (Window *)data;
            for (unsigned long i = 0; i < n && count < MAX_WIN; i++) {
                XWindowAttributes at;
                if (!XGetWindowAttributes(dpy, list[i], &at) || at.map_state != IsViewable) continue;
                struct gwin g;
                memset(&g, 0, sizeof g);
                g.id = (uint32_t)list[i];
                Window child;
                int rx = 0, ry = 0;
                XTranslateCoordinates(dpy, list[i], root, 0, 0, &rx, &ry, &child);
                g.x = rx;
                g.y = ry;
                g.w = at.width;
                g.h = at.height;
                g.flags = 1 | (list[i] == active ? 2 : 0);
                read_title(list[i], g.title, sizeof g.title);
                now[count++] = g;
            }
        }
        if (data) XFree(data);

        /* Tile windows we haven't placed yet, and clamp oversize ones. */
        int is_known[MAX_WIN];
        for (int i = 0; i < count; i++) {
            is_known[i] = 0;
            for (int j = 0; j < nplaced; j++)
                if (placed[j].id == now[i].id) is_known[i] = 1;
        }
        for (int i = 0; i < count; i++) {
            if (is_known[i]) continue;
            unsigned w = now[i].w > (unsigned)sw ? (unsigned)sw : now[i].w;
            unsigned h = now[i].h > (unsigned)sh ? (unsigned)sh : now[i].h;
            /* Space already taken: every window that is placed (or was placed
             * earlier in this pass). */
            struct gwin others[MAX_WIN];
            int no = 0;
            for (int j = 0; j < count; j++)
                if (j != i && (is_known[j] || j < i)) others[no++] = now[j];
            int nx = 0, ny = 0, fits = 0;
            /* Shrink until it fits somewhere without overlapping (never
             * below a usable minimum). */
            for (int pct = 100; pct >= 40 && !fits; pct -= 10) {
                unsigned tw = w * pct / 100, th = h * pct / 100;
                if (tw < 200 || th < 120) break;
                if (find_slot(others, no, tw, th, sw, sh, &nx, &ny)) {
                    w = tw;
                    h = th;
                    fits = 1;
                }
            }
            if (!fits) {
                /* Out of room: re-tile everything instead of overlapping. */
                retile_all(now, count, sw, sh);
                nplaced = 0;
                for (int j = 0; j < count && nplaced < MAX_WIN; j++) placed[nplaced++] = now[j];
                break;
            }
            XMoveResizeWindow(dpy, (Window)now[i].id, nx, ny, w, h);
            now[i].x = nx;
            now[i].y = ny;
            now[i].w = w;
            now[i].h = h;
            if (nplaced < MAX_WIN) placed[nplaced++] = now[i];
        }
        /* Forget windows that are gone. */
        for (int j = 0; j < nplaced;) {
            int alive = 0;
            for (int i = 0; i < count; i++)
                if (now[i].id == placed[j].id) alive = 1;
            if (alive) j++;
            else placed[j] = placed[--nplaced];
        }

        /* Publish only when something changed: every republish bumps the
         * sequence number, which the host reads as "the screen changed" and
         * answers with a full repaint - doing that 16 times a second for an
         * unchanged window list would starve the guest of CPU. */
        if (count != last_count || memcmp(now, last, (size_t)count * sizeof now[0]) != 0) {
            tab->seq++; /* odd: writing */
            __sync_synchronize();
            tab->screen_w = sw;
            tab->screen_h = sh;
            tab->count = count;
            for (int i = 0; i < count; i++) memcpy((void *)&tab->win[i], &now[i], sizeof now[i]);
            __sync_synchronize();
            tab->seq++;
            memcpy(last, now, (size_t)count * sizeof now[0]);
            last_count = count;
        }

        XFlush(dpy);
        usleep(60000);
    }
}
