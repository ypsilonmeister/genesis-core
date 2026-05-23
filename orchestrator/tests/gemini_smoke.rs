// =============================================================================
// gemini_smoke.rs — Gemini CLI の疎通確認テスト
//
// 通常の `cargo test` では skip される (#[ignore])。
// Gemini CLI がインストール済みの環境でのみ実行:
//   cargo test -p orchestrator --test gemini_smoke -- --ignored
// =============================================================================

/// Gemini CLI が起動してレスポンスを返すことを確認する。
/// GEMINI.md Week 2: Gemini API キーの動作確認に対応。
#[test]
#[ignore = "requires gemini CLI to be installed and authenticated"]
fn gemini_cli_responds() {
    let output = std::process::Command::new("gemini")
        .args(["-p", "1+1の答えを数字だけで答えてください"])
        .output()
        .expect("gemini command not found — install Gemini CLI and authenticate");

    assert!(
        output.status.success(),
        "gemini exited with non-zero status: {}\nstderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).expect("gemini output was not valid UTF-8");

    assert!(
        !stdout.trim().is_empty(),
        "gemini returned an empty response"
    );

    println!("gemini response: {}", stdout.trim());
}

/// Gemini CLI のバージョン情報が取得できることを確認する (軽量チェック)。
#[test]
#[ignore = "requires gemini CLI to be installed"]
fn gemini_cli_version() {
    let output = std::process::Command::new("gemini")
        .arg("--version")
        .output()
        .expect("gemini command not found");

    assert!(
        output.status.success(),
        "gemini --version failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}
