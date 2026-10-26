//! Restores the states at start; saves them once they settle.

use embassy_futures::select::{Either, select};
use embassy_time::{Duration, Instant, Timer};
use log::{info, warn};

use super::{Remembered, remembering};
use crate::SharedStore;
use crate::model::{Binding, Entity, changed};

/// After the last change: a slider dragged or a dimmer turned is one write.
const SETTLE: Duration = Duration::from_secs(5);

/// Hands the saved states back to their entities, as commands: call before the app starts.
pub(crate) async fn restore(store: &SharedStore, entities: &[&dyn Entity]) -> Remembered {
    let saved = match store.lock().await.load::<Remembered>().await {
        Ok(saved) => saved.unwrap_or_default(),
        Err(e) => {
            warn!("remember: {e}");
            Remembered::default()
        }
    };
    let n = saved.restore(entities);
    if n > 0 {
        info!("remember: {n} states restored");
    }
    saved
}

/// Whether an entity that remembers changed (sensors wake the binding too).
fn any_changed(entities: &[&dyn Entity]) -> bool {
    remembering(entities).fold(false, |any, e| e.take_dirty(Binding::STORE) | any)
}

#[embassy_executor::task]
pub(crate) async fn remember_task(
    store: &'static SharedStore,
    entities: &'static [&'static dyn Entity],
    mut saved: Remembered,
) {
    loop {
        changed(Binding::STORE).await;
        if !any_changed(entities) {
            continue;
        }
        let mut due = Instant::now() + SETTLE;
        while let Either::First(()) = select(changed(Binding::STORE), Timer::at(due)).await {
            if any_changed(entities) {
                due = Instant::now() + SETTLE;
            }
        }
        if saved.update(entities) {
            match store.lock().await.save(&saved).await {
                Ok(()) => info!("remember: saved"),
                Err(e) => warn!("remember: not saved: {e}"),
            }
        }
    }
}
