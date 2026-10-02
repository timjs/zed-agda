//! The output file: a Markdown stand-in for Emacs's `*Agda information*`
//! buffer, rewritten after every command so Zed reloads it in place.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

pub struct Output {
    pub path: PathBuf,
    shown: AtomicBool,
}

impl Output {
    pub fn new(path: PathBuf) -> Output {
        Output {
            path,
            shown: AtomicBool::new(false),
        }
    }

    /// Replace the file's contents. Returns true the first time, so the caller
    /// can ask Zed to open the file once.
    pub async fn write(&self, title: &str, source: Option<&Path>, body: &str) -> io::Result<bool> {
        let mut text = String::from(
            "<!-- Written by agda-bridge after every Agda command. Edits here are overwritten. -->\n\n",
        );
        text.push_str(&format!("# {title}\n\n"));
        if let Some(source) = source {
            let name = source.file_name().unwrap_or_default().to_string_lossy();
            text.push_str(&format!("*{name}*\n\n"));
        }
        text.push_str(body);

        if let Some(parent) = self.path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        // Write a sibling file and rename it over the old one, so Zed never
        // reads a half-written file.
        let temporary = self.path.with_extension("md.tmp");
        tokio::fs::write(&temporary, text).await?;
        tokio::fs::rename(&temporary, &self.path).await?;
        Ok(!self.shown.swap(true, Ordering::SeqCst))
    }

    /// Create the file if no command has written it yet, so that opening it
    /// shows a file instead of an empty unsaved buffer. Counts as shown.
    pub async fn ensure_exists(&self) -> io::Result<()> {
        if !tokio::fs::try_exists(&self.path).await.unwrap_or(false) {
            self.write(
                "Agda",
                None,
                "No output yet. Save an Agda file to load it.\n",
            )
            .await?;
        }
        self.shown.store(true, Ordering::SeqCst);
        Ok(())
    }
}
