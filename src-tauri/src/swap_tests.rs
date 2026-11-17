use super::*;
use std::fs;
use std::path::PathBuf;

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("horsey-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn swap_replaces_existing_mod() {
    let root = scratch("swap-replace");
    let (staging, dest, old) = (root.join("m.installing"), root.join("m"), root.join("m.old"));
    fs::create_dir_all(&staging).unwrap();
    fs::write(staging.join("new.txt"), b"n").unwrap();
    fs::create_dir_all(&dest).unwrap();
    fs::write(dest.join("old.txt"), b"o").unwrap();
    swap_into_place(&staging, &dest, &old).unwrap();
    assert!(dest.join("new.txt").exists());
    assert!(!dest.join("old.txt").exists());
    assert!(!old.exists());
    assert!(!staging.exists());
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn swap_fresh_install_has_no_old_copy() {
    let root = scratch("swap-fresh");
    let (staging, dest, old) = (root.join("m.installing"), root.join("m"), root.join("m.old"));
    fs::create_dir_all(&staging).unwrap();
    fs::write(staging.join("main.lua"), b"x").unwrap();
    swap_into_place(&staging, &dest, &old).unwrap();
    assert!(dest.join("main.lua").exists());
    assert!(!old.exists());
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn swap_failure_puts_old_mod_back() {
    let root = scratch("swap-fail");
    let (staging, dest, old) = (root.join("m.installing"), root.join("m"), root.join("m.old"));
    // staging fehlt absichtlich, der zweite rename schlägt fehl
    fs::create_dir_all(&dest).unwrap();
    fs::write(dest.join("old.txt"), b"o").unwrap();
    assert!(swap_into_place(&staging, &dest, &old).is_err());
    assert!(dest.join("old.txt").exists());
    assert!(!old.exists());
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn cleanup_removes_half_extracted_installs() {
    let mods = scratch("clean-installing");
    fs::create_dir_all(mods.join("x.installing")).unwrap();
    fs::write(mods.join("x.installing").join("main.lua"), b"x").unwrap();
    clean_mods_dir(&mods);
    assert!(!mods.join("x.installing").exists());
    let _ = fs::remove_dir_all(&mods);
}

#[test]
fn cleanup_restores_old_when_new_copy_is_missing() {
    let mods = scratch("clean-restore");
    fs::create_dir_all(mods.join("y.old")).unwrap();
    fs::write(mods.join("y.old").join("main.lua"), b"y").unwrap();
    clean_mods_dir(&mods);
    assert!(mods.join("y").join("main.lua").exists());
    assert!(!mods.join("y.old").exists());
    let _ = fs::remove_dir_all(&mods);
}

#[test]
fn cleanup_drops_old_when_new_copy_exists() {
    let mods = scratch("clean-drop-old");
    fs::create_dir_all(mods.join("z")).unwrap();
    fs::write(mods.join("z").join("new.txt"), b"n").unwrap();
    fs::create_dir_all(mods.join("z.old")).unwrap();
    fs::write(mods.join("z.old").join("old.txt"), b"o").unwrap();
    clean_mods_dir(&mods);
    assert!(mods.join("z").join("new.txt").exists());
    assert!(!mods.join("z.old").exists());
    let _ = fs::remove_dir_all(&mods);
}

#[test]
fn cleanup_leaves_normal_mods_alone() {
    let mods = scratch("clean-keep");
    fs::create_dir_all(mods.join("keep")).unwrap();
    fs::write(mods.join("keep").join("main.lua"), b"k").unwrap();
    clean_mods_dir(&mods);
    assert!(mods.join("keep").join("main.lua").exists());
    let _ = fs::remove_dir_all(&mods);
}
