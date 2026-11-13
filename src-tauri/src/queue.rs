use crate::submit::Listen;
use std::fs;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

pub const MAX_WAITING: usize = 20_000;

pub struct Queue {
    path: PathBuf,
}

impl Queue {
    pub fn new(dir: &Path) -> Queue {
        Queue {
            path: dir.join("waiting.jsonl"),
        }
    }

    pub fn all(&self) -> Vec<Listen> {
        fs::read_to_string(&self.path)
            .map(|text| {
                text.lines()
                    .filter_map(|line| serde_json::from_str(line).ok())
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn len(&self) -> usize {
        self.all().len()
    }

    fn write(&self, listens: &[Listen]) -> std::io::Result<()> {
        if listens.is_empty() {
            return match fs::remove_file(&self.path) {
                Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error),
                _ => Ok(()),
            };
        }
        if let Some(dir) = self.path.parent() {
            fs::create_dir_all(dir)?;
        }
        let temporary = self.path.with_extension("jsonl.new");
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
        fs::rename(temporary, &self.path)
    }

    pub fn push(&self, listen: Listen) -> std::io::Result<()> {
        let mut listens = self.all();
        listens.push(listen);
        let extra = listens.len().saturating_sub(MAX_WAITING);
        self.write(&listens[extra..])
    }

    pub fn first(&self, count: usize) -> Vec<Listen> {
        self.all().into_iter().take(count).collect()
    }

    pub fn drop_first(&self, count: usize) -> std::io::Result<()> {
        let listens = self.all();
        self.write(&listens[count.min(listens.len())..])
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
