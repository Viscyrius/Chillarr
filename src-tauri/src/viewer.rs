//! Who a request from Jellyfin's web pages is for (bridge.rs). While a bridge
//! request runs, `media::jf`, `media::active_user` and My List answer as that
//! Jellyfin user instead of the profile picked in Chillarr's window, so every
//! command keeps working unchanged for whoever signed in to Jellyfin.
//!
//! It is a task-local: it ends with the request, and a task spawned from one
//! starts without it (as the window's profile) unless it is started through
//! `carry`, which every spawn inside a command that reads the user does.

use crate::jellyfin::Session;
use std::future::Future;

#[derive(Clone, Debug)]
pub struct Viewer {
    /// The viewer's own Jellyfin sign-in: jellyfin-web's token and device id.
    pub session: Session,
    pub admin: bool,
    pub can_delete: bool,
    /// One page load of Chillarr inside Jellyfin; its searches and their
    /// results stay with it.
    pub page: String,
}

tokio::task_local! {
    static VIEWER: Viewer;
}

/// The viewer of the request this code runs for, if it is one from the web.
pub fn current() -> Option<Viewer> {
    VIEWER.try_with(|v| v.clone()).ok()
}

/// Runs `f` as `v`.
pub async fn run<F: Future>(v: Viewer, f: F) -> F::Output {
    VIEWER.scope(v, f).await
}

/// `f`, still as the current viewer (if any) once spawned on its own task.
pub fn carry<F: Future>(f: F) -> impl Future<Output = F::Output> {
    let v = current();
    async move {
        match v {
            Some(v) => VIEWER.scope(v, f).await,
            None => f.await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn viewer(uid: &str) -> Viewer {
        Viewer { session: Session { user_id: uid.into(), ..Default::default() }, admin: false, can_delete: false, page: "p".into() }
    }

    #[tokio::test]
    async fn a_spawned_task_keeps_the_viewer_only_when_carried() {
        run(viewer("u1"), async {
            assert_eq!(current().unwrap().session.user_id, "u1");
            let plain = tokio::spawn(async { current().map(|v| v.session.user_id) }).await.unwrap();
            assert_eq!(plain, None, "task-locals don't cross a spawn by themselves");
            let carried = tokio::spawn(carry(async { current().map(|v| v.session.user_id) })).await.unwrap();
            assert_eq!(carried.as_deref(), Some("u1"));
        })
        .await;
        assert!(current().is_none(), "nothing is left behind after the request");
    }
}
