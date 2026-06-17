//! アプリ共有状態: 最新の所持モジュールと更新時刻。

use crate::optimizer::Module;
use std::sync::{Arc, Mutex};

#[derive(Default)]
pub struct Inner {
    pub modules: Vec<Module>,
    /// 最後にモジュールを更新した unix ms（キャプチャ or ダンプ読込）。
    pub last_update_ms: Option<u64>,
    /// モジュールの取得元（"capture" | "dump" | "none"）。
    pub source: String,
}

pub type SharedState = Arc<Mutex<Inner>>;

pub fn new() -> SharedState {
    Arc::new(Mutex::new(Inner {
        source: "none".to_string(),
        ..Default::default()
    }))
}
