//! A child the engine spawns starts with SIGINT, SIGQUIT and SIGTERM at
//! their default dispositions even when the app runs with them ignored (a
//! background job of a non-interactive shell starts with SIGINT ignored, and
//! an ignored disposition survives exec): `RunningProcess::interrupt` stops
//! an agent turn with SIGINT. This binary holds the one test because it
//! changes the whole test process's dispositions.
#![cfg(target_os = "linux")]

use engine::processes::{Line, spawn_streaming};

#[tokio::test]
async fn children_start_with_default_interrupt_signals() {
    let dir = std::env::temp_dir().join(format!("cad-sig-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    // SAFETY: single-threaded setup before any spawn; signal(2) with
    // SIG_IGN has no handler to race.
    unsafe {
        std::env::set_var("CAD_STUDIO_RUNNING_PROCESSES", dir.join("registry"));
        for sig in [libc::SIGINT, libc::SIGQUIT, libc::SIGTERM] {
            libc::signal(sig, libc::SIG_IGN);
        }
    }
    let argv: Vec<String> = ["/bin/sh", "-c", "grep SigIgn /proc/self/status"]
        .map(String::from)
        .to_vec();
    let env = [("PATH".to_string(), "/usr/bin:/bin".to_string())];
    let mut proc = spawn_streaming(&argv, &dir, &env).unwrap();
    let mut mask = None;
    while let Some(line) = proc.lines.recv().await {
        if let Line::Stdout(text) = line {
            if let Some(hex) = text.strip_prefix("SigIgn:") {
                mask = Some(u64::from_str_radix(hex.trim(), 16).unwrap());
            }
        }
    }
    let mask = mask.expect("the child printed its SigIgn line");
    for (sig, name) in [
        (libc::SIGINT, "INT"),
        (libc::SIGQUIT, "QUIT"),
        (libc::SIGTERM, "TERM"),
    ] {
        assert_eq!(
            mask & (1 << (sig - 1)),
            0,
            "SIG{name} is ignored in the child (SigIgn {mask:#x})"
        );
    }
}
