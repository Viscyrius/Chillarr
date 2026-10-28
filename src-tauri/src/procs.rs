//! Starting the apps. Nothing Chillarr starts ever shows a console window.
//! FlareSolverr is Chillarr's own child, held in a kill-on-close job so it
//! (and the Chrome it drives) goes away when Chillarr quits. Sonarr and
//! qBittorrent are started detached and outlive Chillarr, so downloads are
//! never cut.

use std::os::windows::io::AsRawHandle;
use std::os::windows::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::time::Duration;
use windows::core::{HSTRING, PCWSTR};
use windows::Win32::Foundation::{CloseHandle, ERROR_CANCELLED, HANDLE};
use windows::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation, SetInformationJobObject,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
};
use windows::Win32::Foundation::WAIT_OBJECT_0;
use windows::Win32::System::Threading::{GetExitCodeProcess, WaitForSingleObject};
use windows::Win32::UI::Shell::{ShellExecuteExW, ShellExecuteW, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW};
use windows::Win32::UI::WindowsAndMessaging::{SW_HIDE, SW_SHOWMINNOACTIVE};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const DETACHED_PROCESS: u32 = 0x0000_0008;
/// How long a UAC'd `net stop/start` may take before we stop waiting on it.
const SERVICE_WAIT_MS: u32 = 90_000;

struct Job(HANDLE);
// The job handle is only passed to Win32 calls that are thread-safe.
unsafe impl Send for Job {}
unsafe impl Sync for Job {}

pub struct Procs {
    job: Option<Job>,
    flare: Mutex<Option<Child>>,
}

/// Spawn FlareSolverr only when we don't own one and nothing answers on its port.
pub fn should_spawn(owned: bool, port_open: bool) -> bool {
    !owned && !port_open
}

/// `host:port` from `http://host:port/...`; `None` when there is no explicit port.
pub fn host_port(url: &str) -> Option<String> {
    let rest = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
    let hp = rest.split('/').next()?;
    hp.contains(':').then(|| hp.to_string())
}

pub async fn port_open(base_url: &str) -> bool {
    let Some(hp) = host_port(base_url) else { return false };
    let hp = hp.replacen("localhost", "127.0.0.1", 1);
    matches!(
        tokio::time::timeout(Duration::from_secs(1), tokio::net::TcpStream::connect(hp)).await,
        Ok(Ok(_))
    )
}

pub async fn wait_until(base_url: &str, secs: u64) -> bool {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(secs);
    while tokio::time::Instant::now() < deadline {
        if port_open(base_url).await {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    false
}

impl Procs {
    pub fn new() -> Procs {
        let job = unsafe {
            CreateJobObjectW(None, PCWSTR::null()).ok().map(|h| {
                let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
                info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                let _ = SetInformationJobObject(
                    h,
                    JobObjectExtendedLimitInformation,
                    &info as *const _ as *const _,
                    std::mem::size_of_val(&info) as u32,
                );
                Job(h)
            })
        };
        Procs { job, flare: Mutex::new(None) }
    }

    /// True while the FlareSolverr we started is still alive.
    pub fn owns_flaresolverr(&self) -> bool {
        let mut g = self.flare.lock().unwrap();
        match g.as_mut().map(|c| c.try_wait()) {
            Some(Ok(None)) => true,
            Some(_) => {
                *g = None;
                false
            }
            None => false,
        }
    }

    /// Caller checks `should_spawn` first. A second call while ours is alive is a no-op.
    pub fn start_flaresolverr(&self, exe: &Path) -> Result<(), String> {
        if self.owns_flaresolverr() {
            return Ok(());
        }
        // FlareSolverr passes LANG to its Chrome as --accept-lang. Without it Chrome
        // asks in the PC's own language (Portuguese on a friend's PC), and an
        // unusual language can draw a harder Cloudflare challenge.
        let child = Command::new(exe)
            .env("LANG", "en-US,en")
            .current_dir(exe.parent().unwrap_or(Path::new(".")))
            .creation_flags(CREATE_NO_WINDOW)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("Couldn't start FlareSolverr: {e}"))?;
        if let Some(job) = &self.job {
            unsafe {
                let _ = AssignProcessToJobObject(job.0, HANDLE(child.as_raw_handle()));
            }
        }
        *self.flare.lock().unwrap() = Some(child);
        Ok(())
    }
}

/// An `*arr` app's own tray exe (Sonarr here), told not to open a browser tab.
pub fn start_arr(exe: &Path, name: &str) -> Result<(), String> {
    Command::new(exe)
        .arg("/nobrowser")
        .current_dir(exe.parent().unwrap_or(Path::new(".")))
        .creation_flags(DETACHED_PROCESS | CREATE_NO_WINDOW)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("Couldn't start {name}: {e}"))
}

/// Jellyfin's server without its console, using the same data folder the
/// Windows service uses so the library and accounts are the same. `webdir`
/// is Chillarr's copy of its web pages (jfweb.rs); without it Jellyfin serves
/// its own.
pub fn start_jellyfin(exe: &Path, webdir: Option<&Path>) -> Result<(), String> {
    let data = std::env::var_os("ProgramData").map(std::path::PathBuf::from).unwrap_or_default().join("Jellyfin").join("Server");
    let mut cmd = Command::new(exe);
    cmd.arg("--datadir").arg(data);
    if let Some(w) = webdir {
        cmd.arg("--webdir").arg(w);
    }
    cmd
        .current_dir(exe.parent().unwrap_or(Path::new(".")))
        .creation_flags(DETACHED_PROCESS | CREATE_NO_WINDOW)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("Couldn't start Jellyfin: {e}"))
}

/// qBittorrent opens minimized and does not take focus.
pub fn start_qbit(exe: &Path) -> Result<(), String> {
    let r = unsafe {
        ShellExecuteW(None, &HSTRING::from("open"), &HSTRING::from(exe.as_os_str()), None, None, SW_SHOWMINNOACTIVE)
    };
    // ShellExecuteW returns a value above 32 on success.
    if r.0 as isize > 32 {
        Ok(())
    } else {
        Err(format!("Couldn't start qBittorrent (code {})", r.0 as isize))
    }
}

/// A Windows tool by its full path (`C:\Windows\System32\taskkill.exe`), so
/// a same-named program in the current folder or on PATH is never run instead.
pub fn sys32(exe: &str) -> std::path::PathBuf {
    let root = std::env::var_os("SystemRoot").map(std::path::PathBuf::from).filter(|p| p.is_absolute()).unwrap_or_else(|| r"C:\Windows".into());
    root.join("System32").join(exe)
}

/// Windows PowerShell by its full path.
pub fn powershell() -> std::path::PathBuf {
    sys32(r"WindowsPowerShell\v1.0\powershell.exe")
}

/// A PowerShell single-quoted string. Nothing inside one is special except the
/// quote itself, which PowerShell also accepts in its curly forms.
pub fn ps_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for c in s.chars() {
        if matches!(c, '\'' | '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}') {
            out.push(c);
        }
        out.push(c);
    }
    out.push('\'');
    out
}

/// One PowerShell line that runs a System32 tool with these arguments, each
/// passed as its own quoted string. `$sys` is set by `run_elevated`.
pub fn ps_tool(exe: &str, args: &[&str]) -> String {
    let mut line = format!("& \"$sys\\{exe}\"");
    for a in args {
        line.push(' ');
        line.push_str(&ps_quote(a));
    }
    line
}

/// Runs the lines in order and stops at the first that fails, with its exit code.
pub fn ps_chain(lines: &[String]) -> String {
    let mut out = String::new();
    for l in lines {
        out.push_str(l);
        out.push_str("\nif ($LASTEXITCODE) { exit $LASTEXITCODE }\n");
    }
    out.push_str("exit 0");
    out
}

/// Runs every line, even after one fails; exits with the first failure's code.
pub fn ps_all(lines: &[String]) -> String {
    let mut out = String::from("$rc = 0\n");
    for l in lines {
        out.push_str(l);
        out.push_str("\nif ($LASTEXITCODE -and -not $rc) { $rc = $LASTEXITCODE }\n");
    }
    out.push_str("exit $rc");
    out
}

/// Whether a process with this exe name is running (no window, no console).
pub fn process_running(exe_name: &str) -> bool {
    Command::new(sys32("tasklist.exe"))
        .args(["/FI", &format!("IMAGENAME eq {exe_name}"), "/NH"])
        .creation_flags(CREATE_NO_WINDOW)
        .stdin(Stdio::null())
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).to_ascii_lowercase().contains(&exe_name.to_ascii_lowercase()))
        .unwrap_or(false)
}

/// Closes a program so it can be updated. qBittorrent saves its torrents'
/// progress as it goes, so they resume when it starts again.
pub fn force_close(exe_name: &str) {
    let _ = Command::new(sys32("taskkill.exe"))
        .args(["/F", "/IM", exe_name])
        .creation_flags(CREATE_NO_WINDOW)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

pub fn service_running(name: &str) -> Option<bool> {
    crate::winsvc::query(name)
}

/// Starts (or restarts) a Windows service through one UAC prompt. Only the
/// button calls this; the background guard never does.
pub fn start_service_elevated(name: &str, restart: bool) -> Result<(), String> {
    run_elevated(&service_start_script(name, restart)).map_err(|e| e.replace("{what}", &format!("Starting {name}")))
}

/// A restart's stop may fail (it was half stopped already); the start decides.
fn service_start_script(name: &str, restart: bool) -> String {
    let start = format!("{}\nexit $LASTEXITCODE", ps_tool("net.exe", &["start", name]));
    if restart {
        format!("{}\n{start}", ps_tool("net.exe", &["stop", name]))
    } else {
        start
    }
}

/// The full text PowerShell runs for `run_elevated`: `$sys` is System32 (from
/// Windows itself, not the environment), and anything that throws exits 1.
fn elevated_script(script: &str) -> String {
    format!("$ErrorActionPreference = 'Stop'\n$sys = [Environment]::SystemDirectory\ntry {{\n{script}\n}} catch {{ exit 1 }}")
}

/// `-EncodedCommand` takes the script as base64 UTF-16, so no character in a
/// path or name can end an argument or reach a shell.
fn encoded_args(script: &str) -> String {
    use base64::Engine;
    let utf16: Vec<u8> = elevated_script(script).encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
    format!("-NoProfile -NonInteractive -EncodedCommand {}", base64::engine::general_purpose::STANDARD.encode(utf16))
}

/// Runs a PowerShell script hidden, as admin, through one UAC prompt, and
/// waits for it. Build the script from `ps_tool`/`ps_quote` (never by pasting
/// a path into a command line); its exit code is the result. Errors contain
/// "{what}" for the caller to fill in.
pub fn run_elevated(script: &str) -> Result<(), String> {
    let verb = HSTRING::from("runas");
    let file = HSTRING::from(powershell().as_os_str());
    let params = HSTRING::from(encoded_args(script));
    let mut info = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS,
        lpVerb: PCWSTR(verb.as_ptr()),
        lpFile: PCWSTR(file.as_ptr()),
        lpParameters: PCWSTR(params.as_ptr()),
        nShow: SW_HIDE.0,
        ..Default::default()
    };
    unsafe {
        if let Err(e) = ShellExecuteExW(&mut info) {
            return Err(if e.code() == ERROR_CANCELLED.to_hresult() {
                "{what} needs admin, and the prompt was declined".to_string()
            } else {
                format!("{{what}} failed: {e}")
            });
        }
        if info.hProcess.is_invalid() {
            return Err("{what} didn't start".into());
        }
        let waited = WaitForSingleObject(info.hProcess, SERVICE_WAIT_MS);
        let mut code = 1u32;
        let _ = GetExitCodeProcess(info.hProcess, &mut code);
        let _ = CloseHandle(info.hProcess);
        if waited != WAIT_OBJECT_0 {
            return Err("{what} took too long".into());
        }
        if code != 0 {
            return Err(format!("{{what}} failed (code {code})"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn port_open_detects_listener() {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        assert!(port_open(&format!("http://localhost:{port}")).await);
        drop(l);
        assert!(!port_open(&format!("http://localhost:{port}/x")).await);
    }

    #[test]
    fn flare_not_spawned_when_port_already_open() {
        // Review Focus 4: the user's own FlareSolverr is already up.
        assert!(!should_spawn(false, true));
        assert!(should_spawn(false, false));
        assert!(!should_spawn(true, false));
        assert!(!should_spawn(true, true));
    }

    #[test]
    fn quoting_keeps_every_character_inside_the_string() {
        assert_eq!(ps_quote(r"C:\Program Files\qBittorrent\qbittorrent.exe"), r"'C:\Program Files\qBittorrent\qbittorrent.exe'");
        // cmd's and PowerShell's specials are plain text in a single-quoted string.
        assert_eq!(ps_quote(r#"C:\a" & calc & "%PATH%$(x)`;\b.exe"#), r#"'C:\a" & calc & "%PATH%$(x)`;\b.exe'"#);
        // Only the quote (straight or curly) is doubled.
        assert_eq!(ps_quote("O'Brien"), "'O''Brien'");
        assert_eq!(ps_quote("a\u{2019}; calc; \u{2018}b"), "'a\u{2019}\u{2019}; calc; \u{2018}\u{2018}b'");
    }

    #[test]
    fn tools_run_from_system32_with_each_argument_quoted() {
        assert_eq!(ps_tool("net.exe", &["stop", "Radarr"]), r#"& "$sys\net.exe" 'stop' 'Radarr'"#);
        assert_eq!(service_start_script("Jelly fin", false), "& \"$sys\\net.exe\" 'start' 'Jelly fin'\nexit $LASTEXITCODE");
        assert!(service_start_script("X", true).starts_with("& \"$sys\\net.exe\" 'stop' 'X'\n& \"$sys\\net.exe\" 'start' 'X'"));
        assert_eq!(ps_chain(&["a".into(), "b".into()]), "a\nif ($LASTEXITCODE) { exit $LASTEXITCODE }\nb\nif ($LASTEXITCODE) { exit $LASTEXITCODE }\nexit 0");
        assert!(ps_all(&["a".into()]).ends_with("exit $rc"));
    }

    #[test]
    fn elevated_args_carry_the_script_encoded() {
        use base64::Engine;
        let args = encoded_args("exit 0");
        let b64 = args.strip_prefix("-NoProfile -NonInteractive -EncodedCommand ").unwrap();
        assert!(b64.chars().all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '/' || c == '='));
        let bytes = base64::engine::general_purpose::STANDARD.decode(b64).unwrap();
        let units: Vec<u16> = bytes.chunks(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        let text = String::from_utf16(&units).unwrap();
        assert!(text.contains("$sys = [Environment]::SystemDirectory") && text.contains("exit 0"), "{text}");
        assert!(powershell().ends_with(r"System32\WindowsPowerShell\v1.0\powershell.exe"));
        assert!(sys32("taskkill.exe").is_absolute());
    }

    /// Every elevated script is valid PowerShell. Parsed only, never run.
    #[test]
    fn elevated_scripts_parse() {
        let (svc, path, account) = ("Jelly fin's", r"C:\ProgramData\Jelly'fin\Server", r"PC\O'Neil Smith");
        let scripts = [
            crate::imports::run_as_user_command(svc),
            crate::imports::revert_command(svc),
            crate::imports::jellyfin_as_user_command(svc, path, account),
            crate::imports::revert_jellyfin_command(svc, path, account),
            crate::library::allow_command(&[r"C:\a b\qbittorrent.exe".into(), r#"C:\x' & "%P%"\jellyfin.exe"#.into()]),
            service_start_script(svc, true),
            service_start_script(svc, false),
        ];
        let dir = tempfile::tempdir().unwrap();
        for (i, s) in scripts.iter().enumerate() {
            let f = dir.path().join(format!("s{i}.ps1"));
            std::fs::write(&f, elevated_script(s)).unwrap();
            let check = format!(
                "$e = $null; [void][System.Management.Automation.Language.Parser]::ParseFile({}, [ref]$null, [ref]$e); $e.Count",
                ps_quote(&f.to_string_lossy())
            );
            let out = Command::new(powershell()).args(["-NoProfile", "-NonInteractive", "-Command", &check]).creation_flags(CREATE_NO_WINDOW).output().unwrap();
            assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "0", "script {i} doesn't parse:\n{s}");
        }
    }

    #[test]
    fn host_port_from_url() {
        assert_eq!(host_port("http://localhost:8191"), Some("localhost:8191".into()));
        assert_eq!(host_port("http://localhost:7878/radarr"), Some("localhost:7878".into()));
        assert_eq!(host_port("http://localhost"), None);
    }
}
