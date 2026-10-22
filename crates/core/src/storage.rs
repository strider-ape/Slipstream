//! The output file: one preallocated (sparse) file that every connection writes
//! into at its own offset (SPEC-engine §10).

use std::ffi::OsString;
use std::fs::{File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};

/// `dir/name` → `dir/name.slipstream.part` (the file being written).
pub fn part_path(target: &Path) -> PathBuf {
    with_suffix(target, ".slipstream.part")
}

/// `dir/name` → `dir/name.slipstream.json` (the resume state next to it).
pub fn sidecar_path(target: &Path) -> PathBuf {
    with_suffix(target, ".slipstream.json")
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = OsString::from(path.file_name().unwrap_or_default());
    name.push(suffix);
    path.with_file_name(name)
}

pub struct PartFile {
    file: File,
}

impl PartFile {
    /// Create the part file, or reopen it when resuming (existing bytes are kept).
    /// With a known size the file is made sparse and set to its full length up front.
    pub fn open(path: &Path, size: Option<u64>) -> io::Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)?;
        if let Some(size) = size
            && file.metadata()?.len() != size
        {
            // Best effort: FAT32/exFAT and some network shares have no sparse files;
            // set_len still works there, it's just slower.
            let _ = set_sparse(&file);
            file.set_len(size)?;
        }
        Ok(Self { file })
    }

    /// Write all of `buf` at `offset`, without moving any shared cursor, so many
    /// connections can write into the same file at once.
    pub fn write_at(&self, offset: u64, buf: &[u8]) -> io::Result<()> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::FileExt;
            self.file.write_all_at(buf, offset)
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::FileExt;
            let (mut offset, mut buf) = (offset, buf);
            while !buf.is_empty() {
                let n = self.file.seek_write(buf, offset)?;
                if n == 0 {
                    return Err(io::ErrorKind::WriteZero.into());
                }
                buf = &buf[n..];
                offset += n as u64;
            }
            Ok(())
        }
    }

    /// Push written bytes to the disk. Called before saving progress, so the
    /// sidecar never claims bytes that a power cut could still lose.
    pub fn sync(&self) -> io::Result<()> {
        self.file.sync_data()
    }

    /// Current length on disk (for unknown-size downloads, the bytes written so far).
    pub fn disk_len(&self) -> io::Result<u64> {
        Ok(self.file.metadata()?.len())
    }

    /// Cut the file to `len` bytes (restart of an unknown-size download).
    pub fn truncate(&self, len: u64) -> io::Result<()> {
        self.file.set_len(len)
    }
}

/// On NTFS, writing far past the end makes Windows fill the gap with zeros first,
/// which is slow for big files. A sparse file skips that: unwritten ranges cost
/// nothing. (On Linux, `set_len` already gives a sparse file.)
#[cfg(windows)]
fn set_sparse(file: &File) -> io::Result<()> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::System::IO::DeviceIoControl;
    use windows_sys::Win32::System::Ioctl::FSCTL_SET_SPARSE;

    let mut returned = 0u32;
    // SAFETY: valid open handle; no input/output buffers; synchronous call.
    let ok = unsafe {
        DeviceIoControl(
            file.as_raw_handle(),
            FSCTL_SET_SPARSE,
            std::ptr::null(),
            0,
            std::ptr::null_mut(),
            0,
            &mut returned,
            std::ptr::null_mut(),
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(not(windows))]
fn set_sparse(_file: &File) -> io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("slipstream-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn suffixes() {
        let t = Path::new("d").join("a.zip");
        assert_eq!(part_path(&t), Path::new("d").join("a.zip.slipstream.part"));
        assert_eq!(
            sidecar_path(&t),
            Path::new("d").join("a.zip.slipstream.json")
        );
    }

    #[test]
    fn positional_writes_land_in_place() {
        let dir = temp_dir("storage");
        let path = dir.join("f.part");
        let part = PartFile::open(&path, Some(10)).unwrap();
        part.write_at(6, b"WXYZ").unwrap();
        part.write_at(0, b"ab").unwrap();
        part.sync().unwrap();
        drop(part);
        assert_eq!(std::fs::read(&path).unwrap(), b"ab\0\0\0\0WXYZ");

        // Reopening (resume) keeps what was written.
        let part = PartFile::open(&path, Some(10)).unwrap();
        part.write_at(2, b"cd").unwrap();
        drop(part);
        assert_eq!(std::fs::read(&path).unwrap(), b"abcd\0\0WXYZ");
        std::fs::remove_dir_all(dir).unwrap();
    }
}
