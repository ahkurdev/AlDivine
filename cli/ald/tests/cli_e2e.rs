//! End-to-end test for the `ald` CLI binary.
//!
//! Scaffolds a resource, validates it, and scans a synthetic legacy resource.
//! This is the test that backs the claim "ald new/migrate work".

use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn binary() -> PathBuf {
    // CARGO_BIN_EXE_ald is set by cargo when the package has a bin named ald.
    PathBuf::from(std::env::var("CARGO_BIN_EXE_ald").unwrap_or_else(|_| "target/debug/ald".to_string()))
}

#[test]
fn scaffold_and_validate_roundtrip() {
    let tmp = std::env::temp_dir().join(format!("ald-e2e-{}", std::process::id()));
    fs::create_dir_all(&tmp).unwrap();

    // 1. Scaffold.
    let out = Command::new(binary())
        .args(["new", "e2e-job", tmp.join("e2e-job").to_str().unwrap(), "--lua"])
        .output()
        .expect("ald binary runs");
    assert!(out.status.success(), "new failed: {}", String::from_utf8_lossy(&out.stderr));
    assert!(tmp.join("e2e-job/ald_manifest.toml").exists());
    assert!(tmp.join("e2e-job/server/init.lua").exists());
    assert!(tmp.join("e2e-job/client/init.lua").exists());

    // 2. Validate the scaffolded manifest.
    let out = Command::new(binary())
        .args(["resource", "validate", tmp.join("e2e-job/ald_manifest.toml").to_str().unwrap()])
        .output()
        .expect("ald binary runs");
    assert!(out.status.success(), "validate failed: {}", String::from_utf8_lossy(&out.stderr));
    assert!(String::from_utf8_lossy(&out.stdout).contains("OK"));

    fs::remove_dir_all(&tmp).ok();
}

#[test]
fn migrate_reports_legacy_resource() {
    let tmp = std::env::temp_dir().join(format!("ald-mig-{}", std::process::id()));
    fs::create_dir_all(tmp.join("legacy")).unwrap();
    fs::write(
        tmp.join("legacy/server.lua"),
        "ESX.GetPlayerFromId(1)\nCitizen.CreateThread(function() Citizen.Wait(0) end)\n",
    )
    .unwrap();

    let out = Command::new(binary())
        .args(["migrate", tmp.join("legacy").to_str().unwrap()])
        .output()
        .expect("ald binary runs");

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("detected flavor: esx"), "flavor not detected: {stdout}");
    assert!(stdout.contains("ESX.GetPlayerFromId"), "ESX finding missing: {stdout}");
    assert!(stdout.contains("Citizen.CreateThread"), "Citizen finding missing: {stdout}");
    assert!(stdout.contains("difficulty:"), "no difficulty estimate: {stdout}");
    // Unsupported/FivemOnly findings exit 2 so CI can gate on them.
    assert_eq!(out.status.code(), Some(2));

    fs::remove_dir_all(&tmp).ok();
}

#[test]
fn migrate_clean_resource_is_easy() {
    let tmp = std::env::temp_dir().join(format!("ald-clean-{}", std::process::id()));
    fs::create_dir_all(tmp.join("native/server")).unwrap();
    fs::create_dir_all(tmp.join("native/client")).unwrap();
    fs::write(
        tmp.join("native/ald_manifest.toml"),
        "name = \"clean\"\nversion = \"0.1.0\"\nserver_scripts = [\"server/init.lua\"]\nclient_scripts = [\"client/init.lua\"]\n",
    )
    .unwrap();
    fs::write(tmp.join("native/server/init.lua"), "Aldivine.Events.on('clean:start', function() end)\n").unwrap();

    let out = Command::new(binary())
        .args(["migrate", tmp.join("native").to_str().unwrap()])
        .output()
        .expect("ald binary runs");

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("difficulty: Trivial"), "clean resource should be trivial: {stdout}");
    assert_eq!(out.status.code(), Some(0));

    fs::remove_dir_all(&tmp).ok();
}
