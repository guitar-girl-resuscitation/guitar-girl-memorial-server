use std::{
    path::PathBuf,
    sync::{Arc, Mutex, mpsc},
    thread::JoinHandle,
};

use tokio::sync::oneshot;

use crate::{Database, StoreError};

type Job = Box<dyn FnOnce(&mut Database) + Send + 'static>;

/// Owns the sole read-write SQLite connection on a dedicated thread.
///
/// Android's server runtime can have multiple async request tasks, but every
/// database operation is serialized here. This also keeps rusqlite and its
/// transaction objects out of async task lifetimes.
#[derive(Clone)]
pub struct DatabaseActor {
    inner: Arc<ActorInner>,
}

struct ActorInner {
    sender: Mutex<Option<mpsc::Sender<Job>>>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

impl Drop for ActorInner {
    fn drop(&mut self) {
        self.sender.get_mut().unwrap().take();
        if let Some(thread) = self.thread.get_mut().unwrap().take() {
            let _ = thread.join();
        }
    }
}

impl DatabaseActor {
    pub fn open(path: PathBuf) -> Result<Self, StoreError> {
        let (sender, receiver) = mpsc::channel::<Job>();
        let (ready_sender, ready_receiver) = mpsc::sync_channel(1);
        let thread = std::thread::Builder::new()
            .name("ggfm-sqlite-writer".into())
            .spawn(move || match Database::open(&path) {
                Ok(mut database) => {
                    let _ = ready_sender.send(Ok(()));
                    while let Ok(job) = receiver.recv() {
                        job(&mut database);
                    }
                }
                Err(error) => {
                    let _ = ready_sender.send(Err(error.to_string()));
                }
            })
            .map_err(|error| StoreError::ActorUnavailable(error.to_string()))?;
        ready_receiver
            .recv()
            .map_err(|error| StoreError::ActorUnavailable(error.to_string()))?
            .map_err(StoreError::ActorUnavailable)?;
        Ok(Self {
            inner: Arc::new(ActorInner {
                sender: Mutex::new(Some(sender)),
                thread: Mutex::new(Some(thread)),
            }),
        })
    }

    pub async fn execute<T, F>(&self, operation: F) -> Result<T, StoreError>
    where
        T: Send + 'static,
        F: FnOnce(&mut Database) -> Result<T, StoreError> + Send + 'static,
    {
        let (reply_sender, reply_receiver) = oneshot::channel();
        self.inner
            .sender
            .lock()
            .unwrap()
            .as_ref()
            .ok_or_else(|| StoreError::ActorUnavailable("writer thread stopped".into()))?
            .send(Box::new(move |database| {
                let _ = reply_sender.send(operation(database));
            }))
            .map_err(|_| StoreError::ActorUnavailable("writer thread stopped".into()))?;
        reply_receiver
            .await
            .map_err(|_| StoreError::ActorUnavailable("writer reply was dropped".into()))?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ggfm_domain::CurrencyKind;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[tokio::test]
    async fn serializes_operations_on_one_connection() {
        let path = std::env::temp_dir().join(format!(
            "ggfm-actor-{}.sqlite",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let actor = DatabaseActor::open(path.clone()).unwrap();
        let identity = actor
            .execute(|database| database.create_slot("One", 100))
            .await
            .unwrap();
        let usn = identity.usn;
        actor
            .execute(move |database| {
                database.grant_currency(usn, CurrencyKind::Candy, 3.0, "actor", 101)
            })
            .await
            .unwrap();
        assert_eq!(
            actor
                .execute(move |database| database.currency(usn, CurrencyKind::Candy))
                .await
                .unwrap(),
            3.0
        );
        drop(actor);
        std::fs::remove_file(path).unwrap();
    }
}
