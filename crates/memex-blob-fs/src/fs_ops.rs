use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::Path;

/// A durable put's steps, for naming where a failure happened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Write,
    Fsync,
    Rename,
    DirFsync,
}

/// The file operations a durable put makes. A trait so tests can make any step fail on a real file system.
pub trait FsOps {
    fn write(&self, file: &mut File, bytes: &[u8]) -> io::Result<()>;
    fn fsync(&self, file: &File) -> io::Result<()>;
    fn rename(&self, from: &Path, to: &Path) -> io::Result<()>;
    fn fsync_dir(&self, dir: &Path) -> io::Result<()>;
}

pub struct RealFs;

impl FsOps for RealFs {
    fn write(&self, file: &mut File, bytes: &[u8]) -> io::Result<()> {
        file.write_all(bytes)
    }

    fn fsync(&self, file: &File) -> io::Result<()> {
        file.sync_all()
    }

    fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
        fs::rename(from, to)
    }

    fn fsync_dir(&self, dir: &Path) -> io::Result<()> {
        OpenOptions::new().read(true).open(dir)?.sync_all()
    }
}
