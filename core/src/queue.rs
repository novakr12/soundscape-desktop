use crate::submit::Listen;
use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

pub const MAX_WAITING: usize = 20_000;

static LOCK: Mutex<()> = Mutex::new(());
static CACHED_LEN: Mutex<Option<HashMap<PathBuf, usize>>> = Mutex::new(None);
static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);

fn set_cached_len(path: &Path, len: usize) {
    let mut cache = CACHED_LEN.lock().expect("queue len lock");
    cache
        .get_or_insert_with(HashMap::new)
        .insert(path.to_path_buf(), len);
}

pub struct Queue {
    path: PathBuf,
}

impl Queue {
    pub fn new(dir: &Path) -> Queue {
        Queue {
            path: dir.join("waiting.jsonl"),
        }
    }

    fn read_all(&self) -> Vec<Listen> {
        fs::read_to_string(&self.path)
            .map(|text| {
                text.lines()
                    .filter_map(|line| serde_json::from_str(line).ok())
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn all(&self) -> Vec<Listen> {
        let _guard = LOCK.lock().expect("queue lock");
        self.read_all()
    }

    pub fn len(&self) -> usize {
        let _guard = LOCK.lock().expect("queue lock");
        {
            let cache = CACHED_LEN.lock().expect("queue len lock");
            if let Some(len) = cache.as_ref().and_then(|map| map.get(&self.path).copied()) {
                return len;
            }
        }
        let len = self.read_all().len();
        set_cached_len(&self.path, len);
        len
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn write_locked(&self, listens: &[Listen]) -> std::io::Result<()> {
        if listens.is_empty() {
            let res = match fs::remove_file(&self.path) {
                Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error),
                _ => Ok(()),
            };
            if res.is_ok() {
                set_cached_len(&self.path, 0);
            }
            return res;
        }
        if let Some(dir) = self.path.parent() {
            fs::create_dir_all(dir)?;
        }
        let seq = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
        let temporary = self
            .path
            .with_extension(format!("jsonl.new.{}.{seq}", std::process::id()));
        let written = (|| -> std::io::Result<()> {
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o600)
                .open(&temporary)?;
            for listen in listens {
                writeln!(file, "{}", serde_json::to_string(listen)?)?;
            }
            file.sync_all()?;
            fs::rename(&temporary, &self.path)
        })();
        if written.is_ok() {
            set_cached_len(&self.path, listens.len());
        } else {
            let _ = fs::remove_file(&temporary);
        }
        written
    }

    pub fn push(&self, listen: Listen) -> std::io::Result<()> {
        let _guard = LOCK.lock().expect("queue lock");
        let mut listens = self.read_all();
        listens.push(listen);
        let extra = listens.len().saturating_sub(MAX_WAITING);
        self.write_locked(&listens[extra..])
    }

    pub fn first(&self, count: usize) -> Vec<Listen> {
        let _guard = LOCK.lock().expect("queue lock");
        self.read_all().into_iter().take(count).collect()
    }

    pub fn drop_first(&self, count: usize) -> std::io::Result<()> {
        let _guard = LOCK.lock().expect("queue lock");
        let listens = self.read_all();
        self.write_locked(&listens[count.min(listens.len())..])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scrobble::Track;

    fn listen(n: i64) -> Listen {
        Listen {
            listened_at: n,
            track: Track {
                artist: "A".into(),
                title: format!("Song {n}"),
                album: None,
                length_ms: None,
            },
            player: "p".into(),
        }
    }

    #[test]
    fn keeps_plays_in_order_until_they_are_sent() {
        let dir =
            std::env::temp_dir().join(format!("soundscape-desktop-queue-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let queue = Queue::new(&dir);
        assert_eq!(queue.len(), 0);
        for n in 1..=5 {
            queue.push(listen(n)).unwrap();
        }
        assert_eq!(
            queue
                .first(2)
                .iter()
                .map(|l| l.listened_at)
                .collect::<Vec<_>>(),
            vec![1, 2]
        );
        queue.drop_first(2).unwrap();
        assert_eq!(
            queue
                .all()
                .iter()
                .map(|l| l.listened_at)
                .collect::<Vec<_>>(),
            vec![3, 4, 5]
        );
        queue.drop_first(10).unwrap();
        assert_eq!(queue.len(), 0);
        assert!(!dir.join("waiting.jsonl").exists());
        let _ = fs::remove_dir_all(&dir);
    }
}
