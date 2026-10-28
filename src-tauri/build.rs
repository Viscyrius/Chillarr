use std::path::Path;

/// The oldest TV browser served: LG webOS 4 (2018/19 TVs) is Chromium 53 (the
/// user's TV reports Chrome/53.0.2785.34); webOS 5 is 68, webOS 6 is 79. Our
/// pages die there on syntax like `?.` and `async`, so the copy for older TV
/// browsers is lowered to this; shim.js fills in the missing APIs.
const LEGACY_TARGET: &str = "--target=chrome53";

/// esbuild 0.24.0 as legacy/package-lock.json pins it (integrity-checked by
/// npm ci, no install scripts run), installed into legacy/node_modules once.
fn esbuild_exe() -> Result<std::path::PathBuf, String> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("legacy");
    let platform = match (std::env::consts::OS, std::env::consts::ARCH) {
        ("windows", "x86_64") => "win32-x64",
        ("linux", "x86_64") => "linux-x64",
        ("macos", "aarch64") => "darwin-arm64",
        (os, arch) => return Err(format!("no pinned esbuild for {os}/{arch}")),
    };
    let exe = dir.join("node_modules").join("@esbuild").join(platform).join(if cfg!(windows) { "esbuild.exe" } else { "bin/esbuild" });
    let lock = dir.join("package-lock.json");
    println!("cargo:rerun-if-changed={}", lock.display());
    // What node_modules was installed from: a changed lockfile installs again.
    let want = std::fs::read(&lock).map(|b| format!("{:016x}", fnv1a(&b))).unwrap_or_default();
    let stamp = dir.join("node_modules").join(".chillarr-lock");
    if !exe.is_file() || std::fs::read_to_string(&stamp).ok().as_deref() != Some(want.as_str()) {
        let npm = if cfg!(windows) { "npm.cmd" } else { "npm" };
        let out = std::process::Command::new(npm).args(["ci", "--ignore-scripts", "--no-audit", "--no-fund"]).current_dir(&dir).output().map_err(|e| format!("couldn't run npm: {e}"))?;
        if !out.status.success() || !exe.is_file() {
            return Err(format!("npm ci failed: {}", String::from_utf8_lossy(&out.stderr).lines().last().unwrap_or_default()));
        }
        let _ = std::fs::write(&stamp, &want);
    }
    Ok(exe)
}

/// FNV-1a, enough to notice the lockfile changed (npm ci checks the real hashes).
fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3))
}

fn esbuild(exe: &Path, args: &[&str]) -> Result<(), String> {
    let out = std::process::Command::new(exe).args(args).output().map_err(|e| format!("couldn't run esbuild: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).into_owned())
    }
}

/// legacy.html: the page with the lowered styles and the one bundled script
/// in place of the module, made from index.html so the two can't drift.
fn legacy_page(index: &str) -> Result<String, String> {
    let swaps = [
        ("<html lang=\"en\">", "<html lang=\"en\" class=\"legacy\">"),
        ("<link rel=\"stylesheet\" href=\"styles.css\">", "<link rel=\"stylesheet\" href=\"legacy.css\">"),
        ("<script type=\"module\" src=\"js/app.js\"></script>", "<script src=\"web/legacy.js\"></script>"),
    ];
    let mut page = index.to_string();
    for (from, to) in swaps {
        if page.matches(from).count() != 1 {
            return Err(format!("index.html no longer has exactly one {from}"));
        }
        page = page.replacen(from, to, 1);
    }
    Ok(page)
}

/// The pages for older TV browsers, served as legacy.html (loader.js picks
/// it where `?.` doesn't parse): every script bundled into one classic file,
/// the styles lowered, and the two scripts that run before any choice is made
/// (shim.js, and loader.js inside jellyfin-web) lowered too, so a `?.` added
/// to them later can't break a TV. Release builds always make them and fail
/// without; other builds only when CHILLARR_LEGACY=1 (tests don't need npm).
fn legacy(src: &Path, files: &mut Vec<(String, String)>) {
    println!("cargo:rerun-if-env-changed=CHILLARR_LEGACY");
    let release = std::env::var("PROFILE").as_deref() == Ok("release");
    if !release && std::env::var("CHILLARR_LEGACY").as_deref() != Ok("1") {
        return;
    }
    let out = Path::new(&std::env::var("OUT_DIR").unwrap()).join("legacy");
    let _ = std::fs::create_dir_all(&out);
    let o = |name: &str| out.join(name);
    let made = (|| -> Result<(), String> {
        let exe = esbuild_exe()?;
        let s = |p: &Path| p.to_string_lossy().into_owned();
        esbuild(&exe, &[&s(&src.join("js").join("app.js")), "--bundle", "--format=iife", "--minify", LEGACY_TARGET, &format!("--outfile={}", o("legacy.js").display())])?;
        esbuild(&exe, &[&s(&src.join("styles.css")), "--minify", LEGACY_TARGET, &format!("--outfile={}", o("legacy.css").display())])?;
        for name in ["shim.js", "loader.js"] {
            esbuild(&exe, &[&s(&src.join("web").join(name)), LEGACY_TARGET, &format!("--outfile={}", o(name).display())])?;
        }
        let index = std::fs::read_to_string(src.join("index.html")).map_err(|e| e.to_string())?;
        std::fs::write(o("legacy.html"), legacy_page(&index)?).map_err(|e| e.to_string())?;
        Ok(())
    })();
    if let Err(e) = made {
        if release {
            panic!("the pages for older TVs couldn't be built: {e}");
        }
        println!("cargo:warning=no pages for older TVs in this build: {}", e.lines().next().unwrap_or_default());
        return;
    }
    let path = |name: &str| o(name).to_string_lossy().into_owned();
    // The lowered shim and loader stand in for the originals everywhere.
    files.retain(|(rel, _)| !matches!(rel.as_str(), "web/shim.js" | "web/loader.js" | "legacy.html"));
    for (rel, name) in [("web/legacy.js", "legacy.js"), ("legacy.css", "legacy.css"), ("web/shim.js", "shim.js"), ("web/loader.js", "loader.js"), ("legacy.html", "legacy.html")] {
        files.push((rel.into(), path(name)));
    }
}

/// Lists every file of the window's pages (../src) for jfweb.rs, which copies
/// them into Jellyfin's web pages. Tauri's own embedded copy is compressed and
/// has its HTML rewritten for the window's CSP, so a plain copy is kept here.
fn web_assets() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("src");
    let mut files = Vec::new();
    walk(&src, &src, &mut files);
    legacy(&src, &mut files);
    files.sort();
    let mut out = String::from("pub const WEB_ASSETS: &[(&str, &[u8])] = &[\n");
    for (rel, abs) in &files {
        out.push_str(&format!("    ({rel:?}, include_bytes!({abs:?})),\n"));
    }
    out.push_str("];\n");
    let dest = Path::new(&std::env::var("OUT_DIR").unwrap()).join("web_assets.rs");
    std::fs::write(dest, out).unwrap();
    println!("cargo:rerun-if-changed={}", src.display());
    for (_, abs) in &files {
        println!("cargo:rerun-if-changed={abs}");
    }
}

fn walk(base: &Path, dir: &Path, out: &mut Vec<(String, String)>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            println!("cargo:rerun-if-changed={}", p.display());
            walk(base, &p, out);
        } else {
            let rel = p.strip_prefix(base).unwrap().to_string_lossy().replace('\\', "/");
            out.push((rel, p.to_string_lossy().into_owned()));
        }
    }
}

fn main() {
    web_assets();
    tauri_build::build()
}
