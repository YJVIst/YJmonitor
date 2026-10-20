//! One bounded, shared release check for every browser and monitored node.
//! Checking is automatic; installing remains an authenticated administrator action.
use crate::{backup, core::*, deploy};
use serde_json::{Value, json};
use std::time::Duration;

#[derive(Clone, Default)]
pub struct State {
    pub latest: String,
    pub agent_latest: String,
    pub checked_at: i64,
    pub attempted_at: i64,
    pub checking: bool,
    pub error: String,
}
impl State {
    fn claim(&mut self, manual: bool, at: i64) -> bool {
        let cooldown = if manual {
            60
        } else if self.error.is_empty() {
            3600
        } else {
            300
        };
        if self.checking || (self.attempted_at > 0 && at - self.attempted_at < cooldown) {
            return false;
        }
        self.checking = true;
        self.attempted_at = at;
        true
    }
    pub fn view(&self) -> Value {
        json!({
            "current":VERSION,"latest":self.latest,"agentCurrent":deploy::AGENT_VERSION,
            "agentLatest":self.agent_latest,"available":backup::newer_release(&self.latest, VERSION),
            "agentRequiresPanelUpdate":backup::newer_release(&self.agent_latest, deploy::AGENT_VERSION),
            "checkedAt":self.checked_at,"attemptedAt":self.attempted_at,"checking":self.checking,
            "error":self.error,"stale":self.checked_at == 0 || now()-self.checked_at > 7200 || !self.error.is_empty(),
            "url":if self.latest.is_empty(){String::new()}else{format!("https://github.com/YJVIst/YJmonitor/releases/tag/v{}",self.latest)}
        })
    }
    pub fn validate_update(&self, version: &str) -> ApiResult<()> {
        if !backup::newer_release(version, VERSION) {
            return Err(ApiError::new(409, "当前主控无需升级到此版本"));
        }
        if version != self.latest
            || self.checked_at == 0
            || now() - self.checked_at > 7200
            || !self.error.is_empty()
        {
            return Err(ApiError::new(409, "请先成功检查新版，再提交升级"));
        }
        Ok(())
    }
}
fn parse_latest(raw: &[u8]) -> Result<String, &'static str> {
    let v: Value = serde_json::from_slice(raw).map_err(|_| "发布信息格式不正确")?;
    if v["draft"] != false || v["prerelease"] != false {
        return Err("正式发布信息暂不可用");
    }
    let version = v["tag_name"]
        .as_str()
        .and_then(|s| s.strip_prefix('v'))
        .unwrap_or("");
    if !backup::valid_version(version) {
        return Err("发布版本格式不正确");
    }
    Ok(version.into())
}
async fn fetch(app: &App) -> Result<(String, String), &'static str> {
    let mut response = app
        .0
        .http
        .get("https://api.github.com/repos/YJVIst/YJmonitor/releases/latest")
        .send()
        .await
        .map_err(|_| "GitHub 暂不可用")?;
    if response.status() != 200 || response.content_length().is_some_and(|n| n > 1024 * 1024) {
        return Err("GitHub 查询失败，请稍后重试");
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| "GitHub 响应失败")? {
        if bytes.len() + chunk.len() > 1024 * 1024 {
            return Err("发布信息过大");
        }
        bytes.extend(chunk);
    }
    let latest = parse_latest(&bytes)?;
    let manifest = deploy::fetch_asset(app, &latest, "stable.json", 16384)
        .await
        .map_err(|_| "Agent 发布信息暂不可用")?;
    let agent = deploy::manifest(&manifest).map_err(|_| "Agent 发布签名或信息校验失败")?;
    Ok((latest, agent.version))
}
// Dropping a cancelled HTTP check must not leave the shared checker busy forever.
struct CheckReset(App);
impl Drop for CheckReset {
    fn drop(&mut self) {
        let mut i = self.0.lock();
        if i.updates.checking {
            i.updates.checking = false;
            i.updates.error = "版本检查已中断，稍后重试".into();
        }
    }
}
pub async fn refresh(app: &App, manual: bool) {
    if !app.lock().updates.claim(manual, now()) {
        return;
    }
    let _reset = CheckReset(app.clone());
    // No state lock or worker-thread blocking during DNS/HTTPS/download.
    let result = tokio::time::timeout(Duration::from_secs(20), fetch(app))
        .await
        .unwrap_or(Err("版本检查超时，请稍后重试"));
    let mut i = app.lock();
    let s = &mut i.updates;
    s.checking = false;
    match result {
        Ok((latest, agent)) => {
            s.latest = latest;
            s.agent_latest = agent;
            s.checked_at = now();
            s.error.clear();
        }
        Err(message) => s.error = message.into(),
    }
}
pub fn start(app: &App) {
    let app = app.clone();
    tokio::spawn(async move {
        let mut timer = tokio::time::interval(Duration::from_secs(60));
        timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! { _=app.0.stop.cancelled()=>break, _=timer.tick()=>{} }
            refresh(&app, false).await;
        }
    });
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn deduplicate_checks_and_bound_manual_polling() {
        let mut s = State::default();
        assert!(s.claim(false, 100));
        assert!(!s.claim(true, 200));
        s.checking = false;
        assert!(!s.claim(true, 150));
        assert!(!s.claim(false, 200));
        assert!(s.claim(true, 200));
        s.checking = false;
        s.error = "offline".into();
        assert!(!s.claim(false, 499));
        assert!(s.claim(false, 500));
    }
    #[test]
    fn latest_requires_a_stable_numeric_tag() {
        assert_eq!(
            parse_latest(br#"{"tag_name":"v0.11.5","draft":false,"prerelease":false}"#).unwrap(),
            "0.11.5"
        );
        for raw in [
            br#"{"tag_name":"v0.11.5","draft":true,"prerelease":false}"#.as_slice(),
            br#"{"tag_name":"v0.11.5","draft":false,"prerelease":true}"#.as_slice(),
            br#"{"tag_name":"v0.11.5/evil","draft":false,"prerelease":false}"#.as_slice(),
            br#"{"tag_name":"v0.11.5"}"#.as_slice(),
        ] {
            assert!(parse_latest(raw).is_err());
        }
    }
    #[test]
    fn do_not_offer_reinstall_downgrade_or_unchecked_controller() {
        let mut s = State::default();
        assert!(s.validate_update(VERSION).is_err());
        assert!(s.validate_update("0.1.0").is_err());
        assert!(s.validate_update("99.0.0").is_err());
        s.latest = "99.0.0".into();
        s.checked_at = now();
        assert!(s.validate_update("99.0.0").is_ok());
        assert!(s.validate_update("99.0.1").is_err());
        s.error = "check failed".into();
        assert!(s.validate_update("99.0.0").is_err());
        assert!(s.view()["stale"].as_bool().unwrap());
    }
    #[tokio::test]
    async fn cancelled_check_does_not_block_future_checks() {
        let dir = std::env::temp_dir().join(format!("probe-update-cancel-{}", token()));
        std::fs::create_dir(&dir).unwrap();
        atomic_json(
            &dir.join("auth.json"),
            &crate::model::Auth {
                version: token(),
                ..Default::default()
            },
        )
        .unwrap();
        atomic_json(
            &dir.join("nodes.json"),
            &crate::model::Data {
                schema: 2,
                ..Default::default()
            },
        )
        .unwrap();
        let app = App::new(dir.clone(), "https://updates.test".into(), false).unwrap();
        let pending = async {
            assert!(app.lock().updates.claim(true, now()));
            let _reset = CheckReset(app.clone());
            std::future::pending::<()>().await;
        };
        assert!(
            tokio::time::timeout(Duration::from_millis(1), pending)
                .await
                .is_err()
        );
        let s = &app.lock().updates;
        assert!(!s.checking);
        assert!(s.error.contains("中断"));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
