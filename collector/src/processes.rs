use std::collections::HashMap;
use std::fs;

/// The name an app is tracked under: the file name of the process's executable,
/// so every process of a multi-process app such as a browser shares one name.
pub fn app_name(pid: u32) -> Option<String> {
    if let Ok(executable) = fs::read_link(format!("/proc/{pid}/exe")) {
        let name = executable.file_name()?.to_string_lossy();
        return Some(name.trim_end_matches(" (deleted)").to_owned());
    }
    // Executables of other users' processes are unreadable; their short name is not.
    let short_name = fs::read_to_string(format!("/proc/{pid}/comm")).ok()?;
    Some(short_name.trim_end().to_owned())
}

/// Apps whose processes together hold at least `threshold_bytes` of memory.
pub fn apps_using_at_least(threshold_bytes: u64) -> Vec<String> {
    let Ok(processes) = fs::read_dir("/proc") else {
        return Vec::new();
    };

    let mut bytes_by_app: HashMap<String, u64> = HashMap::new();
    for process in processes.flatten() {
        let Some(pid) = process.file_name().to_str().and_then(|name| name.parse::<u32>().ok()) else {
            continue;
        };
        let bytes = private_memory_bytes(pid);
        if bytes == 0 {
            continue;
        }
        if let Some(app) = app_name(pid) {
            *bytes_by_app.entry(app).or_default() += bytes;
        }
    }

    bytes_by_app.into_iter().filter(|&(_, bytes)| bytes >= threshold_bytes).map(|(app, _)| app).collect()
}

/// Resident memory minus pages shared with other processes. Counting shared pages
/// would bill the same libraries once per process and inflate multi-process apps.
fn private_memory_bytes(pid: u32) -> u64 {
    let Ok(statm) = fs::read_to_string(format!("/proc/{pid}/statm")) else {
        return 0;
    };
    let mut pages = statm.split_ascii_whitespace().skip(1).map(|field| field.parse::<u64>().unwrap_or(0));
    let resident = pages.next().unwrap_or(0);
    let shared = pages.next().unwrap_or(0);

    // SAFETY: sysconf has no preconditions.
    let page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) } as u64;
    resident.saturating_sub(shared) * page_size
}
