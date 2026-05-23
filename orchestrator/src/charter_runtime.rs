// =============================================================================
// charter_runtime.rs — Layer B (charter/enforcement.rs) を実行時に呼び出す薄い橋
//
// charter/enforcement.rs は不可侵領域として配置されている。
// orchestrator はその関数を呼び出すだけで、ロジックを再実装しない。
//
// ビルド時に charter/ をパスとして include! することで、enforcement.rs を
// AI 改変対象外のまま参照する。
// =============================================================================

// Layer B のソースは ../charter/enforcement.rs に置かれている。
// 物理的に同じファイルを参照することで、二重実装による乖離を防ぐ。
#[allow(clippy::trim_split_whitespace)]
#[path = "../../charter/enforcement.rs"]
mod enforcement;

#[allow(unused_imports)]
pub use enforcement::*;

// この時点で `enforce_hard_invariants`, `CharterViolation`, `Action`, `Actor`,
// `IpcChannel`, `EMERGENCY_HALT` がこのモジュールから利用可能になる。
