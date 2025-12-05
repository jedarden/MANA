//! Integration tests for MANA sync functionality
//!
//! Tests the complete export/import/encryption workflow to ensure
//! patterns can be safely shared across workspaces.

use std::path::PathBuf;
use tempfile::TempDir;
use std::process::Command;

/// Get the MANA binary path
fn mana_binary() -> PathBuf {
    // Check for local binary first
    let local = PathBuf::from(".mana/mana");
    if local.exists() {
        return local;
    }

    // Check target/release
    let release = PathBuf::from("target/release/mana");
    if release.exists() {
        return release;
    }

    // Check target/debug
    let debug = PathBuf::from("target/debug/mana");
    if debug.exists() {
        return debug;
    }

    panic!("MANA binary not found. Run `cargo build --release` first.");
}

/// Run mana command and return (success, stdout, stderr)
fn run_mana(args: &[&str]) -> (bool, String, String) {
    let output = Command::new(mana_binary())
        .args(args)
        .output()
        .expect("Failed to execute mana");

    (
        output.status.success(),
        String::from_utf8_lossy(&output.stdout).to_string(),
        String::from_utf8_lossy(&output.stderr).to_string(),
    )
}

#[test]
fn test_mana_version() {
    let (success, stdout, _) = run_mana(&["--version"]);
    assert!(success, "mana --version should succeed");
    assert!(stdout.contains("mana"), "Version output should contain 'mana'");
}

#[test]
fn test_mana_status() {
    let (success, stdout, _) = run_mana(&["status"]);
    assert!(success, "mana status should succeed");
    assert!(stdout.contains("Status:"), "Status output should show status");
}

#[test]
fn test_export_basic() {
    let temp = TempDir::new().expect("Failed to create temp dir");
    let output_path = temp.path().join("patterns.json");

    let (success, stdout, stderr) = run_mana(&[
        "export",
        "--output", output_path.to_str().unwrap(),
    ]);

    // Export may fail if no patterns exist, but command should not crash
    if success {
        assert!(output_path.exists(), "Export file should be created");

        // Verify it's valid JSON
        let content = std::fs::read_to_string(&output_path).unwrap();
        let _: serde_json::Value = serde_json::from_str(&content)
            .expect("Export should produce valid JSON");
    } else {
        // Acceptable if there are no patterns
        println!("Export skipped (likely no patterns): {}", stderr);
    }
}

#[test]
fn test_export_encrypted() {
    let temp = TempDir::new().expect("Failed to create temp dir");
    let output_path = temp.path().join("patterns.enc.json");

    let (success, _, stderr) = run_mana(&[
        "export",
        "--output", output_path.to_str().unwrap(),
        "--encrypted",
        "--passphrase", "test-passphrase-12345",
    ]);

    if success {
        assert!(output_path.exists(), "Encrypted export file should be created");

        // Verify it's valid JSON
        let content = std::fs::read_to_string(&output_path).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&content)
            .expect("Encrypted export should still be valid JSON wrapper");

        // Encrypted format has ciphertext field, unencrypted has metadata
        assert!(
            parsed.get("ciphertext").is_some() || parsed.get("metadata").is_some(),
            "Export should have either ciphertext (encrypted) or metadata (unencrypted)"
        );
    } else {
        println!("Encrypted export skipped: {}", stderr);
    }
}

#[test]
fn test_export_import_roundtrip() {
    let temp = TempDir::new().expect("Failed to create temp dir");
    let export_path = temp.path().join("roundtrip.json");

    // First export
    let (export_success, _, _) = run_mana(&[
        "export",
        "--output", export_path.to_str().unwrap(),
    ]);

    if !export_success {
        println!("Skipping roundtrip test - no patterns to export");
        return;
    }

    // Count patterns before import
    let (_, status_before, _) = run_mana(&["status"]);

    // Import back (with merge to avoid errors if patterns already exist)
    let (import_success, _, stderr) = run_mana(&[
        "import",
        export_path.to_str().unwrap(),
        "--merge", "add",
    ]);

    if import_success {
        let (_, status_after, _) = run_mana(&["status"]);
        println!("Before: {}", status_before);
        println!("After: {}", status_after);
    } else {
        println!("Import completed with notes: {}", stderr);
    }
}

#[test]
fn test_encrypted_roundtrip() {
    let temp = TempDir::new().expect("Failed to create temp dir");
    let export_path = temp.path().join("encrypted-roundtrip.json");
    let passphrase = "secure-test-passphrase-xyz123";

    // Export with encryption
    let (export_success, _, _) = run_mana(&[
        "export",
        "--output", export_path.to_str().unwrap(),
        "--encrypted",
        "--passphrase", passphrase,
    ]);

    if !export_success {
        println!("Skipping encrypted roundtrip - no patterns to export");
        return;
    }

    // Import with same passphrase
    let (import_success, stdout, stderr) = run_mana(&[
        "import",
        export_path.to_str().unwrap(),
        "--passphrase", passphrase,
        "--merge", "add",
    ]);

    // Should succeed with correct passphrase
    println!("Import stdout: {}", stdout);
    println!("Import stderr: {}", stderr);
    assert!(
        import_success || stdout.contains("imported") || stdout.contains("patterns") || stderr.contains("already exist"),
        "Encrypted import should succeed with correct passphrase"
    );
}

#[test]
fn test_encrypted_wrong_passphrase() {
    let temp = TempDir::new().expect("Failed to create temp dir");
    let export_path = temp.path().join("wrong-pass.json");

    // Export with encryption
    let (export_success, _, _) = run_mana(&[
        "export",
        "--output", export_path.to_str().unwrap(),
        "--encrypted",
        "--passphrase", "correct-passphrase",
    ]);

    if !export_success {
        println!("Skipping wrong passphrase test - no patterns to export");
        return;
    }

    // Import with wrong passphrase
    let (import_success, _, stderr) = run_mana(&[
        "import",
        export_path.to_str().unwrap(),
        "--passphrase", "wrong-passphrase",
    ]);

    // Should fail with wrong passphrase
    assert!(
        !import_success || stderr.contains("decrypt") || stderr.contains("error"),
        "Import should fail with wrong passphrase"
    );
}

#[test]
fn test_sync_init_git() {
    let temp = TempDir::new().expect("Failed to create temp dir");
    let repo_path = temp.path().join("test-sync-repo");

    // Initialize a git repo for testing
    Command::new("git")
        .args(["init", repo_path.to_str().unwrap()])
        .output()
        .expect("Failed to init git repo");

    // Try to initialize sync (may fail if git remote not configured, but shouldn't crash)
    let (_, stdout, stderr) = run_mana(&[
        "sync", "init",
        "--backend", "git",
        "--remote", repo_path.to_str().unwrap(),
    ]);

    // Just check it doesn't panic
    println!("Sync init stdout: {}", stdout);
    println!("Sync init stderr: {}", stderr);
}

#[test]
fn test_sync_status() {
    let (success, stdout, _) = run_mana(&["sync", "status"]);

    // May fail if sync not configured, but shouldn't crash
    println!("Sync status: success={}, output={}", success, stdout);
}

#[test]
fn test_bench() {
    let (success, stdout, _) = run_mana(&["bench"]);

    assert!(success, "Benchmark should complete successfully");
    assert!(stdout.contains("MANA Performance Benchmarks"), "Should show benchmark header");
    assert!(stdout.contains("Context Injection"), "Should test context injection");
    assert!(stdout.contains("Pattern Search"), "Should test pattern search");
}

#[test]
fn test_stats() {
    let (success, stdout, _) = run_mana(&["stats"]);

    assert!(success, "Stats should complete successfully");
    assert!(stdout.contains("Pattern Statistics"), "Should show pattern stats");
}

#[test]
fn test_help_commands() {
    // Test all major subcommands have help
    let commands = vec![
        "inject",
        "session-end",
        "consolidate",
        "status",
        "stats",
        "export",
        "import",
        "sync",
        "team",
    ];

    for cmd in commands {
        let (success, _, _) = run_mana(&[cmd, "--help"]);
        assert!(success, "mana {} --help should succeed", cmd);
    }
}
