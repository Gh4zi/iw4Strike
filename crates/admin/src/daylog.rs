//! `admin/logs/YYYY-MM-DD.log`: one plain-text file a day of joins, leaves, chat and admin
//! actions, readable without any tool.

use std::io::Write;
use std::path::PathBuf;

pub(crate) struct DayLog {
    dir: PathBuf,
    open: Option<(String, std::fs::File)>,
}

impl DayLog {
    pub(crate) fn new(dir: PathBuf) -> Self {
        Self { dir, open: None }
    }

    pub(crate) fn line(&mut self, text: &str) {
        let now = chrono::Local::now();
        let day = now.format("%Y-%m-%d").to_string();
        if self.open.as_ref().is_none_or(|(open, _)| *open != day) {
            self.open = std::fs::create_dir_all(&self.dir)
                .and_then(|()| {
                    std::fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(self.dir.join(format!("{day}.log")))
                })
                .map(|file| (day, file))
                .inspect_err(|error| diag::warn!(Net, "admin log: {error}"))
                .ok();
        }
        if let Some((_, file)) = self.open.as_mut() {
            let _ = writeln!(file, "[{}] {text}", now.format("%H:%M:%S"));
        }
    }
}
