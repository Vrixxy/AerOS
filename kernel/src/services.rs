//! Service table behind the `svc` shell command: named programs that can be
//! started, stopped and inspected. The table only tracks state; launching and
//! reaping go through the shell and scheduler.

use crate::sync::TicketLock;

pub const MAX_SERVICES: usize = 8;
pub const NAME_BYTES: usize = 24;
pub const PATH_BYTES: usize = 96;

#[derive(Clone, Copy)]
pub struct Service {
    name: [u8; NAME_BYTES],
    name_length: usize,
    path: [u8; PATH_BYTES],
    path_length: usize,
    pub task_id: Option<u64>,
    pub last_exit: Option<u64>,
    pub starts: u32,
}

impl Service {
    pub fn name(&self) -> &str {
        core::str::from_utf8(&self.name[..self.name_length]).unwrap_or("")
    }

    pub fn path(&self) -> &str {
        core::str::from_utf8(&self.path[..self.path_length]).unwrap_or("")
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ServiceError {
    InvalidName,
    PathTooLong,
    Exists,
    TableFull,
    Unknown,
}

static SERVICES: TicketLock<[Option<Service>; MAX_SERVICES]> =
    TicketLock::new([None; MAX_SERVICES]);

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= NAME_BYTES
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
}

pub fn add(name: &str, path: &str) -> Result<(), ServiceError> {
    if !valid_name(name) {
        return Err(ServiceError::InvalidName);
    }
    if path.is_empty() || path.len() > PATH_BYTES {
        return Err(ServiceError::PathTooLong);
    }
    let mut table = SERVICES.lock();
    if table.iter().flatten().any(|service| service.name() == name) {
        return Err(ServiceError::Exists);
    }
    let Some(slot) = table.iter().position(Option::is_none) else {
        return Err(ServiceError::TableFull);
    };
    let mut service = Service {
        name: [0; NAME_BYTES],
        name_length: name.len(),
        path: [0; PATH_BYTES],
        path_length: path.len(),
        task_id: None,
        last_exit: None,
        starts: 0,
    };
    service.name[..name.len()].copy_from_slice(name.as_bytes());
    service.path[..path.len()].copy_from_slice(path.as_bytes());
    table[slot] = Some(service);
    Ok(())
}

pub fn remove(name: &str) -> Result<(), ServiceError> {
    let mut table = SERVICES.lock();
    let slot = table
        .iter()
        .position(|entry| entry.is_some_and(|service| service.name() == name))
        .ok_or(ServiceError::Unknown)?;
    table[slot] = None;
    Ok(())
}

pub fn get(name: &str) -> Option<Service> {
    SERVICES
        .lock()
        .iter()
        .flatten()
        .find(|service| service.name() == name)
        .copied()
}

pub fn update(name: &str, change: impl FnOnce(&mut Service)) -> Result<(), ServiceError> {
    let mut table = SERVICES.lock();
    let service = table
        .iter_mut()
        .flatten()
        .find(|service| service.name() == name)
        .ok_or(ServiceError::Unknown)?;
    change(service);
    Ok(())
}

pub fn for_each(mut visit: impl FnMut(&Service)) {
    for service in SERVICES.lock().iter().flatten() {
        visit(service);
    }
}

pub fn self_test() -> bool {
    let names = ["svc-probe-a", "svc-probe-b"];
    for name in names {
        let _ = remove(name);
    }
    let added = add(names[0], "/bin/init").is_ok();
    let duplicate_rejected = add(names[0], "/bin/other") == Err(ServiceError::Exists);
    let bad_name_rejected = add("bad name", "/bin/init") == Err(ServiceError::InvalidName);
    let long_bytes = [b'/'; PATH_BYTES + 1];
    let long_path = core::str::from_utf8(&long_bytes).unwrap_or("");
    let long_path_rejected = add(names[1], long_path) == Err(ServiceError::PathTooLong);
    let updated = update(names[0], |service| {
        service.task_id = Some(7);
        service.starts += 1;
    })
    .is_ok();
    let stored = get(names[0]).is_some_and(|service| {
        service.path() == "/bin/init" && service.task_id == Some(7) && service.starts == 1
    });
    let mut listed = 0;
    for_each(|service| {
        if service.name() == names[0] {
            listed += 1;
        }
    });
    let removed = remove(names[0]).is_ok();
    let gone = get(names[0]).is_none() && remove(names[0]) == Err(ServiceError::Unknown);
    added
        && duplicate_rejected
        && bad_name_rejected
        && long_path_rejected
        && updated
        && stored
        && listed == 1
        && removed
        && gone
}
