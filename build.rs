use std::{env, fs, path::PathBuf, process::Command};

fn main() {
    println!("cargo:rerun-if-changed=app.rc");
    println!("cargo:rerun-if-changed=assets/icon.ico");
    let rc = env::split_paths(&env::var_os("PATH").unwrap_or_default())
        .map(|path| path.join("rc.exe")).find(|path| path.is_file())
        .or_else(|| {
            let bin = PathBuf::from(env::var_os("ProgramFiles(x86)")?).join("Windows Kits/10/bin");
            fs::read_dir(bin).ok()?.filter_map(Result::ok)
                .map(|entry| entry.path().join("x64/rc.exe")).find(|path| path.is_file())
        }).expect("Windows SDK Resource Compiler (rc.exe) is required to embed the app icon");
    let resource = PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("app.res");
    let status = Command::new(rc).current_dir(env::var_os("CARGO_MANIFEST_DIR").unwrap())
        .arg("/nologo").arg("/fo").arg(&resource).arg("app.rc").status()
        .expect("Cannot run the Windows resource compiler");
    assert!(status.success(), "Cannot compile app.rc");
    println!("cargo:rustc-link-arg={}", resource.display());
}
