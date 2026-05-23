// =============================================================================
// Lying Calculator — System Charter Layer B (Enforcement Layer)
//
// 重要: 本ファイルは AI による直接改変対象外。
// 改変は人間が charter/system.md と同時に Tier 3 プロセスで行う。
//
// このファイルは Layer A (system.md) の Hard Invariants を実行時に物理的に
// 強制する規則の集合体である。本コミット時点ではスケルトンであり、
// 創成期 Week 1 で最小実装、Week 2 以降で段階的に拡充する。
// =============================================================================

#![allow(dead_code)]

use std::path::Path;

/// 憲章違反の種別。Layer A §2 の Hard Invariants に 1:1 対応する。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CharterViolation {
    /// HI-1: 計算結果が数学的に正しくない (サイレント誤答)
    SilentMathError {
        input: String,
        produced: String,
        expected: String,
    },
    /// HI-2: モジュールがオーケストレータを経由せず直接通信した
    DirectModuleCommunication { from: String, to: String },
    /// HI-3: メタデータの削除または更新を試みた
    MetadataMutation { table: String, operation: String },
    /// HI-4: 攻撃 AI がソースコード領域に触れた
    AttackAiCodeAccess { path: String },
    /// HI-5: AI が charter/ 配下のファイルを書き換えようとした
    CharterFileWrite { path: String },
}

/// 実行アクターの種別。Layer B は actor によって許される操作が変わる。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Actor {
    Human,
    Orchestrator,
    Module,
    RepairAi,
    AttackAi,
}

/// オーケストレータが各アクションの直前に呼び出すゲート。
/// Ok を返したアクションのみが実行される。
pub fn enforce_hard_invariants(
    actor: Actor,
    action: &Action,
) -> Result<(), CharterViolation> {
    // HI-5: charter/ 配下への書き込みは Human 以外禁止
    if let Action::FileWrite { path, .. } = action {
        if is_charter_path(path) && actor != Actor::Human {
            return Err(CharterViolation::CharterFileWrite {
                path: path.display().to_string(),
            });
        }
    }

    // HI-4: 攻撃 AI のコードアクセス禁止
    if actor == Actor::AttackAi {
        if let Action::FileRead { path } | Action::FileWrite { path, .. } = action {
            if is_source_code_path(path) {
                return Err(CharterViolation::AttackAiCodeAccess {
                    path: path.display().to_string(),
                });
            }
        }
    }

    // HI-3: メタデータの削除・更新禁止
    if let Action::DbOperation { sql, .. } = action {
        if is_destructive_sql(sql) {
            return Err(CharterViolation::MetadataMutation {
                table: extract_table_name(sql).unwrap_or_else(|| "unknown".into()),
                operation: extract_sql_verb(sql).unwrap_or_else(|| "unknown".into()),
            });
        }
    }

    // HI-2: モジュール間直接通信禁止
    if let Action::Ipc { from, to, channel } = action {
        if *channel == IpcChannel::DirectSocket && actor == Actor::Module {
            return Err(CharterViolation::DirectModuleCommunication {
                from: from.clone(),
                to: to.clone(),
            });
        }
    }

    // HI-1 のサイレント誤答検出は実行後に評価する。
    // ここでは Skeleton として placeholder のみ残す。

    Ok(())
}

/// 緊急停止フラグ。CMP ループ暴走時に true にするとオーケストレータが停止する。
/// このフラグはハードコードであり、ランタイムからは変更できない。
pub const EMERGENCY_HALT: bool = false;

// -----------------------------------------------------------------------------
// 内部ヘルパ
// -----------------------------------------------------------------------------

/// 改変禁止パス: charter/ 配下
fn is_charter_path(path: &Path) -> bool {
    path.components()
        .any(|c| c.as_os_str() == "charter")
}

/// 攻撃 AI から隔離するパス: charter/, modules/, orchestrator/
fn is_source_code_path(path: &Path) -> bool {
    const PROTECTED: &[&str] = &["charter", "modules", "orchestrator", "Cargo.toml"];
    path.components()
        .any(|c| PROTECTED.iter().any(|p| c.as_os_str() == *p))
}

/// 破壊的 SQL の検出 (DELETE / UPDATE / DROP / TRUNCATE / ALTER)
fn is_destructive_sql(sql: &str) -> bool {
    const VERBS: &[&str] = &["DELETE", "UPDATE", "DROP", "TRUNCATE", "ALTER"];
    let upper = sql.trim_start().to_uppercase();
    VERBS.iter().any(|v| upper.starts_with(v))
}

fn extract_sql_verb(sql: &str) -> Option<String> {
    sql.trim_start()
        .split_whitespace()
        .next()
        .map(|s| s.to_uppercase())
}

fn extract_table_name(_sql: &str) -> Option<String> {
    // TODO: 簡易パースで FROM / INTO の次の identifier を返す
    None
}

// -----------------------------------------------------------------------------
// Action 型 (オーケストレータが Layer B に渡す抽象操作)
// -----------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub enum Action {
    FileRead {
        path: std::path::PathBuf,
    },
    FileWrite {
        path: std::path::PathBuf,
        size_bytes: usize,
    },
    DbOperation {
        sql: String,
    },
    Ipc {
        from: String,
        to: String,
        channel: IpcChannel,
    },
    SpawnProcess {
        binary: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IpcChannel {
    /// オーケストレータ経由の通常チャネル
    Orchestrated,
    /// モジュール同士の直接ソケット (禁止)
    DirectSocket,
}

// -----------------------------------------------------------------------------
// 試験コード (Week 1 で実装拡張)
// -----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn ai_cannot_write_to_charter() {
        let action = Action::FileWrite {
            path: PathBuf::from("charter/system.md"),
            size_bytes: 100,
        };
        let result = enforce_hard_invariants(Actor::RepairAi, &action);
        assert!(matches!(
            result,
            Err(CharterViolation::CharterFileWrite { .. })
        ));
    }

    #[test]
    fn human_can_write_to_charter() {
        let action = Action::FileWrite {
            path: PathBuf::from("charter/system.md"),
            size_bytes: 100,
        };
        assert!(enforce_hard_invariants(Actor::Human, &action).is_ok());
    }

    #[test]
    fn attack_ai_cannot_read_module_source() {
        let action = Action::FileRead {
            path: PathBuf::from("modules/normalizer/src/main.rs"),
        };
        let result = enforce_hard_invariants(Actor::AttackAi, &action);
        assert!(matches!(
            result,
            Err(CharterViolation::AttackAiCodeAccess { .. })
        ));
    }

    #[test]
    fn destructive_sql_is_rejected() {
        let action = Action::DbOperation {
            sql: "DELETE FROM modifications WHERE id = 1".into(),
        };
        let result = enforce_hard_invariants(Actor::Orchestrator, &action);
        assert!(matches!(
            result,
            Err(CharterViolation::MetadataMutation { .. })
        ));
    }
}
