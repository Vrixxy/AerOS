document.documentElement.classList.add("js");

const reduce = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
const fine = window.matchMedia("(hover: hover) and (pointer: fine)").matches;
const BOOT_DELAY = reduce || document.documentElement.classList.contains("no-boot") ? 0 : 1700;
const $ = (q, el = document) => el.querySelector(q);
const $$ = (q, el = document) => [...el.querySelectorAll(q)];

/* Nav, menu, scroll progress */
const nav = $(".nav");
const menu = $(".menu");
const bar = $(".progress");
const rail = $(".steps-rail span");
const steps = $$(".step");

function onScroll() {
  const y = window.scrollY;
  nav.classList.toggle("scrolled", y > 8);
  const max = document.documentElement.scrollHeight - innerHeight;
  bar.style.transform = `scaleX(${max > 0 ? y / max : 0})`;

  if (rail) {
    const box = rail.parentElement.getBoundingClientRect();
    const t = Math.min(1, Math.max(0, (innerHeight * 0.75 - box.top) / (innerHeight * 0.6)));
    rail.style.transform = `scaleX(${t})`;
    steps.forEach((step, i) => step.classList.toggle("lit", t >= i / steps.length + 0.02 || t === 1));
  }
}
window.addEventListener("scroll", onScroll, { passive: true });
window.addEventListener("resize", onScroll);
onScroll();

menu.addEventListener("click", () => {
  const open = nav.classList.toggle("open");
  menu.setAttribute("aria-expanded", String(open));
});
$$(".nav-links a").forEach((a) =>
  a.addEventListener("click", () => {
    nav.classList.remove("open");
    menu.setAttribute("aria-expanded", "false");
  })
);

/* Headline: words rise in one after another */
const title = $(".display");
if (title && !reduce) {
  const walk = (node) => {
    [...node.childNodes].forEach((child) => {
      if (child.nodeType === 3) {
        const frag = document.createDocumentFragment();
        child.textContent.split(/(\s+)/).forEach((part) => {
          if (!part) return;
          if (/^\s+$/.test(part)) return frag.append(part);
          const w = document.createElement("span");
          w.className = "w";
          const inner = document.createElement("span");
          inner.textContent = part;
          w.append(inner);
          frag.append(w);
        });
        child.replaceWith(frag);
      }
    });
  };
  walk(title);
  $$(".w > span", title).forEach((s, i) => (s.style.transitionDelay = `${120 + i * 90}ms`));
  title.classList.remove("reveal");
  setTimeout(() => requestAnimationFrame(() => title.classList.add("split-in")), BOOT_DELAY + 30);
}

/* The hero laptop rises in on load, the rest as it scrolls into view */
setTimeout(() => $(".hero-shot").classList.add("in"), reduce ? 0 : BOOT_DELAY + 650);

/* Reveal on scroll */
const reveals = $$(".reveal");
if ("IntersectionObserver" in window && !reduce) {
  const io = new IntersectionObserver(
    (entries) =>
      entries.forEach((e) => {
        if (!e.isIntersecting) return;
        const sibs = [...e.target.parentElement.children].filter((el) => el.classList.contains("reveal"));
        e.target.style.transitionDelay = `${Math.min(sibs.indexOf(e.target), 5) * 70}ms`;
        e.target.classList.add("in");
        io.unobserve(e.target);
      }),
    { threshold: 0.12, rootMargin: "0px 0px -40px 0px" }
  );
  reveals.forEach((el) => io.observe(el));
} else {
  reveals.forEach((el) => el.classList.add("in"));
}

/* Hero: screen tabs that cycle on their own */
const tabs = $$(".tab");
const shots = $$(".shot");
const tabBar = $(".tabs");
const CYCLE = 5000;
let current = 0;
let timer = null;
tabBar.style.setProperty("--cycle", `${CYCLE}ms`);

function show(i, user) {
  current = (i + shots.length) % shots.length;
  shots.forEach((s, n) => s.classList.toggle("is-active", n === current));
  tabs.forEach((t, n) => {
    const on = n === current;
    t.classList.toggle("is-active", on);
    t.setAttribute("aria-selected", String(on));
    const fill = $("i", t);
    fill.style.animation = "none";
    void fill.offsetWidth;
    fill.style.animation = "";
  });
  clearTimeout(timer);
  if (!reduce) timer = setTimeout(() => show(current + 1), user ? CYCLE * 1.6 : CYCLE);
}
tabs.forEach((t, n) => t.addEventListener("click", () => show(n, true)));
show(0);

/* Hero: the laptop leans toward the pointer */
const device = $("[data-tilt]");
if (device && fine && !reduce) {
  const stage = $(".hero");
  stage.addEventListener("pointermove", (e) => {
    const r = device.getBoundingClientRect();
    const x = (e.clientX - (r.left + r.width / 2)) / r.width;
    const y = (e.clientY - (r.top + r.height / 2)) / r.height;
    device.style.transform = `rotateY(${x * 7}deg) rotateX(${-y * 5}deg)`;
  });
  stage.addEventListener("pointerleave", () => (device.style.transform = ""));
}

/* Cards tilt slightly under the pointer */
if (fine && !reduce) {
  $$(".card, .device-card").forEach((card) => {
    card.addEventListener("pointermove", (e) => {
      const r = card.getBoundingClientRect();
      const x = (e.clientX - r.left) / r.width - 0.5;
      const y = (e.clientY - r.top) / r.height - 0.5;
      card.style.transition = "transform .15s ease-out";
      card.style.transform = `perspective(900px) rotateY(${x * 4}deg) rotateX(${-y * 4}deg) translateY(-3px)`;
    });
    card.addEventListener("pointerleave", () => {
      card.style.transition = "transform .6s cubic-bezier(.2,.7,.1,1)";
      card.style.transform = "";
    });
  });

  /* Buttons lean toward the cursor */
  $$(".btn-dark, .btn-lg, .verdict-cta").forEach((btn) => {
    btn.addEventListener("pointermove", (e) => {
      const r = btn.getBoundingClientRect();
      const x = e.clientX - (r.left + r.width / 2);
      const y = e.clientY - (r.top + r.height / 2);
      btn.style.transform = `translate(${x * 0.18}px, ${y * 0.3}px)`;
    });
    btn.addEventListener("pointerleave", () => (btn.style.transform = ""));
  });
}

/* Numbers count up when they come into view */
const counters = $$("[data-count]");
const countIO = new IntersectionObserver((entries) =>
  entries.forEach((e) => {
    if (!e.isIntersecting) return;
    countIO.unobserve(e.target);
    const end = +e.target.dataset.count;
    if (reduce) return;
    const start = performance.now();
    const tick = (now) => {
      const t = Math.min(1, (now - start) / 1400);
      e.target.textContent = Math.round(end * (1 - Math.pow(1 - t, 4)));
      if (t < 1) requestAnimationFrame(tick);
    };
    requestAnimationFrame(tick);
  })
);
counters.forEach((c) => countIO.observe(c));

/* Terminal types itself out, then loops */
const term = $(".term code");
if (term) {
  const lines = term.dataset.type.split("|");
  const caret = '<span class="caret"></span>';
  const esc = (s) => s.replace(/&/g, "&amp;").replace(/</g, "&lt;");
  let done = [];
  const render = (partial = "") => {
    term.innerHTML = done.join("\n") + (done.length ? "\n" : "") + esc(partial) + caret;
  };
  const run = async () => {
    const wait = (ms) => new Promise((r) => setTimeout(r, ms));
    for (;;) {
      done = [];
      for (const line of lines) {
        if (line.startsWith("$")) {
          for (let i = 1; i <= line.length; i++) {
            render(line.slice(0, i));
            await wait(reduce ? 0 : 45 + Math.random() * 50);
          }
          done.push(esc(line));
          await wait(320);
        } else {
          done.push(`<span class="out">${esc(line)}</span>`);
          render();
          await wait(520);
        }
      }
      render();
      await wait(3800);
    }
  };
  const termIO = new IntersectionObserver((entries) => {
    if (entries[0].isIntersecting) {
      termIO.disconnect();
      run();
    }
  });
  termIO.observe(term);
}


const PLAIN_NOTES = {
  "Macs are not supported.": "AerOS doesn't run on Macs.",
  "ARM processors are not supported. AerOS needs a 64-bit x86 CPU.": "This kind of laptop chip isn't supported. AerOS needs an AMD or Intel processor.",
  "AerOS needs UEFI firmware. Legacy BIOS PCs can't start it.": "Your PC is too old to start AerOS.",
  "The installer runs on Windows 10 or 11.": "You need Windows 10 or 11 to run the installer.",
  "Check in Windows: System Information, BIOS Mode should say UEFI.": "To check: search Windows for System Information. If BIOS Mode says UEFI, you're good.",
  "The desktop runs. Linux apps need AMD-V, so they're not available on Intel yet.": "AerOS works, but Linux apps need an AMD processor for now.",
  "AMD-V lets you run Linux apps. Turn on SVM in your firmware if it's off.": "You can run Linux apps too. If they don't start, a setting called SVM needs turning on in your PC's startup settings.",
  "Linux apps run best with 8 GB or more.": "Linux apps run better with 8 GB of memory or more.",
  "32 GB or more leaves room for apps and files.": "32 GB or more leaves room for your apps and files.",
  "Turn off Secure Boot once after setup. The installer shows you how.": "You'll switch off one security setting after installing. The installer walks you through it.",
};
/* Compatibility checker */
const form = $(".check-form");
if (form) {
  const verdict = $(".verdict");
  const state = { cpu: "amd", fw: "uefi", win: "11", ram: 8, disk: 64 };
  const paths = {
    ok: '<path d="M6 12.5l4 4 8-9"/>',
    warn: '<path d="M12 7v6M12 16.5v.5"/>',
    no: '<path d="M7 7l10 10M17 7L7 17"/>',
  };

  const slider = (input) => {
    const pct = ((input.value - input.min) / (input.max - input.min)) * 100;
    input.style.setProperty("--fill", `${pct}%`);
  };

  function evaluate() {
    const blockers = [];
    const notes = [];
    if (state.cpu === "mac") blockers.push("Macs are not supported.");
    if (state.cpu === "arm") blockers.push("ARM processors are not supported. AerOS needs a 64-bit x86 CPU.");
    if (state.fw === "bios") blockers.push("AerOS needs UEFI firmware. Legacy BIOS PCs can't start it.");
    if (state.win === "none") blockers.push("The installer runs on Windows 10 or 11.");
    if (state.ram < 4) blockers.push(`${state.ram} GB of memory is below the 4 GB minimum.`);
    if (state.disk < 16) blockers.push(`${state.disk} GB free is below the 16 GB AerOS needs.`);

    if (state.fw === "unsure") notes.push("Check in Windows: System Information, BIOS Mode should say UEFI.");
    if (state.cpu === "intel") notes.push("The desktop runs. Linux apps need AMD-V, so they're not available on Intel yet.");
    if (state.cpu === "amd") notes.push("AMD-V lets you run Linux apps. Turn on SVM in your firmware if it's off.");
    if (state.ram >= 4 && state.ram < 8) notes.push("Linux apps run best with 8 GB or more.");
    if (state.disk >= 16 && state.disk < 32) notes.push("32 GB or more leaves room for apps and files.");
    notes.push("Turn off Secure Boot once after setup. The installer shows you how.");

    let kind = "ok";
    let titleText = "Ready to install";
    let list = notes;
    if (blockers.length) {
      kind = "no";
      titleText = "Not supported";
      list = blockers;
    } else if (state.fw === "unsure" || state.cpu === "intel" || state.ram < 8) {
      kind = "warn";
      titleText = state.fw === "unsure" ? "Probably works" : "Works, with limits";
    }

    const changed = !verdict.classList.contains(kind) || $(".verdict-title", verdict).textContent !== titleText;
    verdict.classList.remove("ok", "warn", "no", "pop");
    verdict.classList.add(kind);
    $(".verdict-mark svg", verdict).innerHTML = paths[kind];
    $(".verdict-title", verdict).textContent = titleText;
    const plainOn = document.documentElement.classList.contains("plain-on");
    $(".verdict-list", verdict).innerHTML = list.map((t) => `<li>${plainOn ? PLAIN_NOTES[t] || t : t}</li>`).join("");
    if (changed && !reduce) {
      void verdict.offsetWidth;
      verdict.classList.add("pop");
    }
  }

  $$(".seg", form).forEach((seg) =>
    seg.addEventListener("click", (e) => {
      const b = e.target.closest("button");
      if (!b) return;
      $$("button", seg).forEach((x) => x.classList.toggle("on", x === b));
      state[seg.dataset.key] = b.dataset.v;
      evaluate();
    })
  );
  $$("input[type=range]", form).forEach((input) => {
    const out = $(`[data-out="${input.dataset.key}"]`, form);
    const update = () => {
      state[input.dataset.key] = +input.value;
      out.textContent = `${input.value} GB`;
      slider(input);
      evaluate();
    };
    input.addEventListener("input", update);
    update();
  });
  evaluate();
  window.recheck = evaluate;
}

/* Hardware list: filter by status and search */
const grid = $(".compat-grid");
if (grid) {
  const items = $$(".compat-item", grid);
  items.forEach((it) => {
    const pill = $(".pill", it);
    it.dataset.s = pill.classList.contains("pill-yes") ? "yes" : pill.classList.contains("pill-exp") ? "exp" : "no";
  });
  const empty = document.createElement("p");
  empty.className = "compat-empty";
  empty.textContent = "Nothing matches. Try another search.";
  let filter = "all";
  const search = $(".search");
  const apply = () => {
    const q = search.value.trim().toLowerCase();
    let shown = 0;
    items.forEach((it) => {
      const match = (filter === "all" || it.dataset.s === filter) && it.textContent.toLowerCase().includes(q);
      const was = !it.classList.contains("hide");
      it.classList.toggle("hide", !match);
      if (match) {
        shown++;
        if (!was && !reduce) {
          it.classList.remove("flash");
          void it.offsetWidth;
          it.classList.add("flash");
        }
      }
    });
    if (!shown) grid.append(empty);
    else empty.remove();
  };
  $$(".chip").forEach((chip) =>
    chip.addEventListener("click", () => {
      filter = chip.dataset.f;
      $$(".chip").forEach((c) => c.classList.toggle("on", c === chip));
      apply();
    })
  );
  search.addEventListener("input", apply);
}

/* Theme: toggle with a circular reveal from the button */
const root = document.documentElement;
const themeMeta = $('meta[name="theme-color"]');
function applyTheme(t) {
  root.setAttribute("data-theme", t);
  themeMeta.setAttribute("content", t === "dark" ? "#000000" : "#ffffff");
  $$(".theme").forEach((b) => b.setAttribute("aria-label", t === "dark" ? "Switch to light mode" : "Switch to dark mode"));
  try {
    localStorage.setItem("aeros-theme", t);
  } catch (e) {}
}
applyTheme(root.getAttribute("data-theme") || "light");
$$(".theme").forEach((btn) =>
  btn.addEventListener("click", () => {
    const next = root.getAttribute("data-theme") === "dark" ? "light" : "dark";
    if (!document.startViewTransition || reduce) return applyTheme(next);
    const r = btn.getBoundingClientRect();
    const x = r.left + r.width / 2;
    const y = r.top + r.height / 2;
    const end = Math.hypot(Math.max(x, innerWidth - x), Math.max(y, innerHeight - y));
    document.startViewTransition(() => applyTheme(next)).ready.then(() =>
      root.animate(
        { clipPath: [`circle(0px at ${x}px ${y}px)`, `circle(${end}px at ${x}px ${y}px)`] },
        { duration: 650, easing: "cubic-bezier(.2,.7,.1,1)", pseudoElement: "::view-transition-new(root)" }
      )
    );
  })
);

/* Hero: the laptop starts tipped back and settles as you scroll */
const stage = $(".tilt-stage");
const statement = $(".statement-text");
const toTop = $(".to-top");
let words = [];
if (statement && !reduce) {
  statement.innerHTML = statement.textContent
    .split(/\s+/)
    .map((w) => `<span class="sw">${w}</span>`)
    .join(" ");
  words = $$(".sw", statement);
}
function scrollFx() {
  const y = window.scrollY;
  if (stage && !reduce) {
    const t = Math.min(1, y / (innerHeight * 0.55));
    stage.style.transform = `perspective(1600px) rotateX(${(1 - t) * 16}deg) scale(${0.9 + t * 0.1})`;
  }
  if (words.length) {
    const r = statement.getBoundingClientRect();
    const t = Math.min(1, Math.max(0, (innerHeight * 0.85 - r.top) / (r.height + innerHeight * 0.35)));
    const lit = Math.round(t * words.length);
    words.forEach((w, i) => w.classList.toggle("lit", i < lit));
  }
  toTop.classList.toggle("show", y > innerHeight * 1.5);
}
window.addEventListener("scroll", scrollFx, { passive: true });
window.addEventListener("resize", scrollFx);
scrollFx();

/* Section labels decode when they appear */
const glyphs = "ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789/#*";
function scramble(el) {
  const final = el.textContent;
  let frame = 0;
  const total = 22;
  el.classList.add("scramble");
  const tick = () => {
    frame++;
    el.textContent = [...final]
      .map((ch, i) => {
        if (ch === " " || i < (frame / total) * final.length) return ch;
        return glyphs[(Math.random() * glyphs.length) | 0];
      })
      .join("");
    if (frame < total) requestAnimationFrame(tick);
    else el.textContent = final;
  };
  requestAnimationFrame(tick);
}
if (!reduce) {
  const kio = new IntersectionObserver((entries) =>
    entries.forEach((e) => {
      if (!e.isIntersecting) return;
      kio.unobserve(e.target);
      setTimeout(() => scramble(e.target), 150);
    }),
    { threshold: 1 }
  );
  $$(".kicker").forEach((k) => kio.observe(k));
}

/* Footer wordmark letters rise in */
const mark = $(".wordmark");
if (mark) {
  $$("span", mark).forEach((s, i) => (s.style.transitionDelay = `${i * 70}ms`));
  new IntersectionObserver((entries, o) => {
    if (entries[0].isIntersecting) {
      mark.classList.add("in");
      o.disconnect();
    }
  }, { threshold: 0.3 }).observe(mark);
}

/* Download buttons acknowledge the click */
$$('a[href$="AerOS-Setup.exe"]').forEach((a) =>
  a.addEventListener("click", () => {
    if (a.dataset.busy) return;
    a.dataset.busy = "1";
    const html = a.innerHTML;
    a.innerHTML = '<svg class="ico" viewBox="0 0 24 24"><path d="M6 12.5l4 4 8-9"/></svg>Starting download';
    setTimeout(() => {
      a.innerHTML = html;
      delete a.dataset.busy;
    }, 3200);
  })
);

/* Plain English: swap technical wording for everyday language */
const plainButtons = $$(".plain");
const plainEls = $$("[data-plain]");
plainEls.forEach((el) => (el.dataset.nerd = el.innerHTML));
const toast = $(".toast");
let toastTimer = null;
function say(text) {
  toast.textContent = text;
  toast.classList.add("show");
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => toast.classList.remove("show"), 2200);
}
function setPlain(on, animate) {
  const swap = () => {
    plainEls.forEach((el) => {
      el.innerHTML = on ? el.dataset.plain : el.dataset.nerd;
      el.classList.remove("fresh");
      if (on && animate) {
        void el.offsetWidth;
        el.classList.add("fresh");
      }
    });
    root.classList.toggle("plain-on", on);
    plainButtons.forEach((b) => b.setAttribute("aria-pressed", String(on)));
    if (window.recheck) window.recheck();
    try {
      localStorage.setItem("aeros-plain", on ? "1" : "0");
    } catch (e) {}
  };
  if (!animate || reduce) return swap();
  root.classList.add("swapping");
  setTimeout(() => {
    swap();
    root.classList.remove("swapping");
  }, 180);
  say(on ? "Showing plain English" : "Showing technical details");
}
let startPlain = false;
try {
  startPlain = localStorage.getItem("aeros-plain") === "1";
} catch (e) {}
if (startPlain) setPlain(true, false);
plainButtons.forEach((b) =>
  b.addEventListener("click", () => setPlain(!root.classList.contains("plain-on"), true))
);

/* Boot intro: shown once per visit */
try {
  sessionStorage.setItem("aeros-boot", "1");
} catch (e) {}

/* The AerOS cursor: follows the pointer exactly, inverts over anything clickable */
const pointer = $(".cursor");
if (pointer && fine && !reduce) {
  root.classList.add("cursor-ready");
  let lastX = 0, tilt = 0, x = -100, y = -100;
  const place = () => {
    pointer.style.transform = `translate(${x}px, ${y}px) rotate(${tilt}deg)`;
  };
  window.addEventListener("pointermove", (e) => {
    if (e.pointerType !== "mouse") return;
    x = e.clientX;
    y = e.clientY;
    tilt += (Math.max(-8, Math.min(8, (x - lastX) * 0.6)) - tilt) * 0.35;
    lastX = x;
    pointer.classList.add("on");
    const t = e.target instanceof Element ? e.target : null;
    pointer.classList.toggle("text", !!(t && t.closest('input[type="search"], input[type="text"], textarea')));
    pointer.classList.toggle("hover", !!(t && t.closest("a, button, summary, label, .tab, .chip, .seg button, input[type=range]")));
    place();
  });
  const settle = () => {
    tilt *= 0.85;
    if (Math.abs(tilt) > 0.05) place();
    requestAnimationFrame(settle);
  };
  settle();
  document.documentElement.addEventListener("pointerleave", () => pointer.classList.remove("on"));
  window.addEventListener("pointerdown", () => pointer.classList.add("down"));
  window.addEventListener("pointerup", () => pointer.classList.remove("down"));
}

/* Nav: a soft pill slides to the link under the pointer */
const links = $(".nav-links");
if (links && fine) {
  const pill = document.createElement("span");
  pill.className = "nav-pill";
  links.prepend(pill);
  $$("a", links).forEach((a) =>
    a.addEventListener("pointerenter", () => {
      pill.style.left = `${a.offsetLeft - 12}px`;
      pill.style.width = `${a.offsetWidth + 24}px`;
    })
  );
}

/* Buttons ripple from where you click */
$$(".btn").forEach((btn) =>
  btn.addEventListener("pointerdown", (e) => {
    if (reduce) return;
    const r = btn.getBoundingClientRect();
    const size = Math.max(r.width, r.height) * 2.2;
    const dot = document.createElement("span");
    dot.className = "ripple";
    dot.style.width = dot.style.height = `${size}px`;
    dot.style.left = `${e.clientX - r.left - size / 2}px`;
    dot.style.top = `${e.clientY - r.top - size / 2}px`;
    btn.append(dot);
    setTimeout(() => dot.remove(), 750);
  })
);

/* Type "aeros" anywhere: it rains leaves */
let typed = "";
function leafRain() {
  if (reduce) return say("🍃");
  for (let i = 0; i < 46; i++) {
    const leaf = document.createElement("span");
    leaf.className = "leaf-drop";
    const size = 22 + Math.random() * 38;
    leaf.style.left = `${Math.random() * 100}vw`;
    leaf.style.width = `${size}px`;
    leaf.style.height = `${size * 0.67}px`;
    leaf.style.opacity = (0.35 + Math.random() * 0.65).toFixed(2);
    leaf.style.setProperty("--dx", `${(Math.random() - 0.5) * 240}px`);
    leaf.style.setProperty("--spin", `${(Math.random() - 0.5) * 900}deg`);
    leaf.style.animationDuration = `${2.4 + Math.random() * 2.2}s`;
    leaf.style.animationDelay = `${Math.random() * 1.2}s`;
    leaf.innerHTML = '<svg><use href="#leaf"/></svg>';
    document.body.append(leaf);
    setTimeout(() => leaf.remove(), 6000);
  }
  say("Hello from AerOS");
}
window.addEventListener("keydown", (e) => {
  if (e.target instanceof Element && e.target.closest("input, textarea")) return;
  if (e.key.length !== 1) return;
  typed = (typed + e.key.toLowerCase()).slice(-5);
  if (typed === "aeros") {
    typed = "";
    leafRain();
  }
});
