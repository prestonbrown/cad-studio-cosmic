//! Process supervision tests (task 6). Every spawn points the registry at a
//! per-test temp file via the `CAD_STUDIO_RUNNING_PROCESSES` override so the
//! real `~/.local/share/cad-studio/running-processes` is never touched.

use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use engine::processes::{Line, reap_orphans_at, register_at, registry_path, spawn_streaming};

const ENV_VAR: &str = "CAD_STUDIO_RUNNING_PROCESSES";
const MINIMAL_ENV: &[(&str, &str)] = &[("PATH", "/usr/bin:/bin")];

// std::env::set_var is process-global and these tests run in parallel
// threads: every [set → spawn] window and every registry_path() read is
// taken under this lock. After spawn_streaming returns, the registry path is
// captured inside RunningProcess, so later writes cannot land in the wrong
// file even once the lock is released.
static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn tmp_dir() -> PathBuf {
    let d = std::env::temp_dir().join(format!("cad-procs-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn write_script(dir: &Path, name: &str, body: &str) -> PathBuf {
    let p = dir.join(name);
    std::fs::write(&p, body).unwrap();
    p
}

/// Write `script.sh` and spawn `/bin/sh script.sh` with the registry pointed
/// at `dir/running-processes`.
async fn spawn_script(dir: &Path, body: &str) -> engine::processes::RunningProcess {
    let script = write_script(dir, "script.sh", body);
    let argv = vec!["/bin/sh".to_string(), script.to_string_lossy().into_owned()];
    let env: Vec<(String, String)> = MINIMAL_ENV
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    // SAFETY: all setters and readers of ENV_VAR hold ENV_LOCK.
    unsafe { std::env::set_var(ENV_VAR, dir.join("running-processes")) };
    spawn_streaming(&argv, dir, &env).unwrap()
}

fn alive(pid: u32) -> bool {
    Command::new("/bin/kill")
        .arg("-0")
        .arg(pid.to_string())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn ppid_of(pid: u32) -> Option<u32> {
    let out = Command::new("/bin/ps")
        .args(["-o", "ppid=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    String::from_utf8_lossy(&out.stdout).trim().parse().ok()
}

async fn eventually(what: impl Fn() -> bool, within: Duration) -> bool {
    let deadline = Instant::now() + within;
    loop {
        if what() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn stdout_and_stderr_stream_line_by_line() {
    let dir = tmp_dir();
    let body = "echo out-1\necho out-2\necho out-3\necho err-1 >&2\nexit 0\n";
    let mut proc = spawn_script(&dir, body).await;

    let mut stdout_lines = Vec::new();
    let mut stderr_lines = Vec::new();
    while let Some(line) = proc.lines.recv().await {
        match line {
            Line::Stdout(l) => stdout_lines.push(l),
            Line::Stderr(l) => stderr_lines.push(l),
        }
    }
    let stderr_text = proc.stderr_text.await.unwrap();
    let status = proc.exit_status.await.unwrap();

    assert_eq!(
        stdout_lines,
        ["out-1", "out-2", "out-3"],
        "per-stream order preserved"
    );
    assert_eq!(stderr_lines, ["err-1"]);
    assert_eq!(stderr_text, "err-1\n");
    assert!(status.success());
}

#[tokio::test(flavor = "multi_thread")]
async fn interrupt_escalates_to_sigterm() {
    let dir = tmp_dir();
    // The script ignores SIGINT and would sleep for 30 s: only the 3 s
    // SIGTERM escalation can end it. The sleep child is a leftover by design
    // (signals go to the child's pid only), so its pid is recorded for cleanup.
    let pidfile = dir.join("leftover.pid");
    let body = format!(
        "trap '' INT\nsleep 30 &\necho $! > '{}'\nwait\n",
        pidfile.display()
    );
    let proc = spawn_script(&dir, &body).await;

    let registry = dir.join("running-processes");
    let marker = format!("{}:", proc.pid);
    assert!(
        std::fs::read_to_string(&registry)
            .unwrap()
            .contains(&marker),
        "spawn registers the pid"
    );

    let started = Instant::now();
    proc.interrupt();
    let status = tokio::time::timeout(Duration::from_secs(5), proc.exit_status)
        .await
        .expect("exits within ~5 s via the SIGTERM path")
        .unwrap();
    let elapsed = started.elapsed();
    assert!(
        elapsed >= Duration::from_secs(3),
        "SIGINT was ignored, so nothing but the 3 s escalation could end it"
    );
    assert!(elapsed < Duration::from_secs(5));
    assert_eq!(status.signal(), Some(15), "killed by SIGTERM, not SIGINT");
    assert!(
        !std::fs::read_to_string(&registry)
            .unwrap()
            .contains(&marker),
        "exit unregisters the pid"
    );

    let leftover: u32 = std::fs::read_to_string(&pidfile)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    let _ = Command::new("/bin/kill")
        .arg("-9")
        .arg(leftover.to_string())
        .status();
}

#[tokio::test(flavor = "multi_thread")]
async fn gentle_interrupt_kills_cooperative_script() {
    let dir = tmp_dir();
    // The trap runs between sleep ticks, so SIGINT lands within one tick and
    // each tick's sleep is short enough to outlive the script by 0.2 s at most.
    let body = "trap 'exit 7' INT\nwhile true; do sleep 0.2; done\n";
    let proc = spawn_script(&dir, body).await;

    let started = Instant::now();
    proc.interrupt();
    let status = tokio::time::timeout(Duration::from_secs(2), proc.exit_status)
        .await
        .expect("cooperative script exits quickly on SIGINT alone")
        .unwrap();
    assert!(started.elapsed() < Duration::from_secs(2));
    assert_eq!(
        status.code(),
        Some(7),
        "exit status reaches the consumer: the trap's own code, not a signal"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn stdin_is_dev_null() {
    let dir = tmp_dir();
    let argv = vec!["/bin/cat".to_string()];
    let env: Vec<(String, String)> = MINIMAL_ENV
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    // SAFETY: all setters and readers of ENV_VAR hold ENV_LOCK.
    unsafe { std::env::set_var(ENV_VAR, dir.join("running-processes")) };
    let mut proc = spawn_streaming(&argv, &dir, &env).unwrap();
    drop(_guard);

    // cat with no stdin attached must see immediate EOF, not hang: the whole
    // pipeline (channel close + exit) is capped, not just the exit wait.
    let (lines, status) = tokio::time::timeout(Duration::from_secs(5), async {
        let mut lines = Vec::new();
        while let Some(line) = proc.lines.recv().await {
            lines.push(line);
        }
        let status = proc.exit_status.await.unwrap();
        (lines, status)
    })
    .await
    .expect("cat on null stdin completes immediately");

    assert!(lines.is_empty());
    assert!(status.success());
}

#[tokio::test(flavor = "multi_thread")]
async fn orphan_reaping() {
    let dir = tmp_dir();
    let registry = dir.join("running-processes");

    // Double-fork without C: the wrapper exits at once, so its background
    // sleep is re-parented to init (ppid 1) — exactly the crash leftover
    // reap_orphans exists for.
    let spawn_orphan = |pidfile: &Path| {
        Command::new("/bin/sh")
            .arg("-c")
            .arg(format!(
                "sleep 300 & echo $! > '{}'; exit 0",
                pidfile.display()
            ))
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();
    };
    let orphan_pidfile = dir.join("orphan.pid");
    let forged_pidfile = dir.join("forged.pid");
    spawn_orphan(&orphan_pidfile);
    spawn_orphan(&forged_pidfile);
    let orphan: u32 = std::fs::read_to_string(&orphan_pidfile)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    let forged: u32 = std::fs::read_to_string(&forged_pidfile)
        .unwrap()
        .trim()
        .parse()
        .unwrap();

    // Control: our own live child — ppid is this test process, never 1.
    let mut control: Child = Command::new("/bin/sleep")
        .arg("300")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();

    assert!(
        eventually(|| ppid_of(orphan) == Some(1), Duration::from_secs(2)).await,
        "wrapper exit re-parents the sleep to init"
    );
    assert!(eventually(|| ppid_of(forged) == Some(1), Duration::from_secs(2)).await);

    register_at(&registry, orphan);
    register_at(&registry, control.id());
    let reaped = reap_orphans_at(&registry);
    assert_eq!(
        reaped,
        vec![orphan],
        "only the ppid-1 entry matches and dies"
    );
    assert!(
        eventually(|| !alive(orphan), Duration::from_secs(2)).await,
        "reaped orphan is SIGTERMed"
    );
    assert!(alive(control.id()), "a live non-orphan child is untouched");
    assert!(
        std::fs::read_to_string(&registry)
            .unwrap()
            .starts_with(&format!("{}:", control.id())),
        "a live child of this process stays listed after the reap"
    );

    // Pid-recycling defense: a ppid-1 process whose registry line carries a
    // start time that is not its own is not ours to kill.
    assert!(eventually(|| ppid_of(forged) == Some(1), Duration::from_secs(2)).await);
    std::fs::write(&registry, format!("{forged}:WRONG-START")).unwrap();
    let reaped = reap_orphans_at(&registry);
    assert!(reaped.is_empty(), "starttime mismatch spares the process");
    assert!(alive(forged));
    assert!(
        std::fs::read_to_string(&registry)
            .unwrap()
            .starts_with(&format!("{}:", forged)),
        "a spared entry that still resolves stays listed, its line re-derived from ps"
    );

    control.kill().unwrap();
    control.wait().unwrap();
    let _ = Command::new("/bin/kill")
        .arg("-9")
        .arg(forged.to_string())
        .status();
    assert!(
        eventually(|| !alive(forged), Duration::from_secs(2)).await,
        "init reaps the SIGKILLed leftover before the final reap"
    );
    // both entries now dead: persist drops pids ps no longer resolves
    assert!(reap_orphans_at(&registry).is_empty());
    assert_eq!(
        std::fs::read_to_string(&registry).unwrap(),
        "",
        "registry drops dead entries"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn reap_keeps_live_children_listed() {
    let dir = tmp_dir();
    let registry = dir.join("running-processes");
    // our OWN live child; the trap makes the cleanup SIGINT land within one
    // sleep tick (the gentle-interrupt pattern), so the awaited exit below
    // cannot stall on an untrapped shell waiting out its sleep
    let proc = spawn_script(&dir, "trap 'exit 7' INT\nwhile true; do sleep 0.2; done\n").await;
    // (the spawn already registered the pid — no register_at needed)
    let killed = reap_orphans_at(&registry);
    assert!(killed.is_empty(), "ppid != 1: not touched");
    let text = std::fs::read_to_string(&registry).unwrap();
    assert!(
        text.contains(&format!("{}:", proc.pid)),
        "live child erased from registry: {text}"
    );
    proc.interrupt();
    // AWAIT the exit before reaping: SIGINT is asynchronous, and a
    // still-live child would legitimately stay listed.
    let _ = tokio::time::timeout(Duration::from_secs(5), proc.exit_status).await;
    let _ = reap_orphans_at(&registry);
    assert_eq!(
        std::fs::read_to_string(&registry).unwrap(),
        "",
        "gone after death: no longer listed"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn registry_survives_and_deduplicates() {
    let dir = tmp_dir();
    let registry = dir.join("running-processes");
    let me = std::process::id();

    register_at(&registry, me);
    register_at(&registry, me);

    let text = std::fs::read_to_string(&registry).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(
        lines.len(),
        1,
        "registering a known pid twice leaves one line: {text:?}"
    );
    assert!(
        lines[0].starts_with(&format!("{me}:")),
        "line is pid:starttime: {text:?}"
    );

    {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // SAFETY: all setters and readers of ENV_VAR hold ENV_LOCK.
        unsafe { std::env::set_var(ENV_VAR, &registry) };
        assert_eq!(
            registry_path(),
            registry,
            "override selects the registry file"
        );
    }
}
