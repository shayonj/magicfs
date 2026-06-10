use fuser::{
    FileAttr, FileType, Filesystem, MountOption, ReplyAttr, ReplyData, ReplyDirectory,
    ReplyEntry, ReplyOpen, ReplyWrite, Request, TimeOrNow,
};
use std::ffi::OsStr;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const TTL: Duration = Duration::from_secs(1);

const ROOT: u64 = 1;
const HELLO: u64 = 2;
const TIME: u64 = 3;
const WEATHER: u64 = 4;
const NOTES: u64 = 5;

const FILES: &[(u64, &str)] = &[
    (HELLO, "hello.txt"),
    (TIME, "time.txt"),
    (WEATHER, "weather.txt"),
    (NOTES, "notes.txt"),
];

const HELLO_TEXT: &str = "Hello! I am not a real file. A tiny Rust program made me up.\n";
const WEATHER_TEXT: &str = "Tomorrow: sunny, a high of 22, light wind, and no rain.\n";

struct MagicFS {
    weather_cached: bool,
    notes: Vec<u8>,
}

impl MagicFS {
    fn contents(&mut self, ino: u64) -> String {
        match ino {
            HELLO => HELLO_TEXT.to_string(),
            TIME => format!("The time right now is {}\n", chrono::Local::now().format("%H:%M:%S")),
            NOTES => String::from_utf8_lossy(&self.notes).into_owned(),
            WEATHER => {
                if !self.weather_cached {
                    eprintln!("[magicfs] weather.txt: first read, simulating a slow download...");
                    std::thread::sleep(Duration::from_secs(2));
                    eprintln!("[magicfs] weather.txt: downloaded, cached in memory");
                    self.weather_cached = true;
                }
                WEATHER_TEXT.to_string()
            }
            _ => String::new(),
        }
    }

    fn attr(&self, ino: u64) -> FileAttr {
        let (kind, perm, size) = match ino {
            ROOT => (FileType::Directory, 0o755, 0),
            HELLO => (FileType::RegularFile, 0o444, HELLO_TEXT.len() as u64),
            TIME => (FileType::RegularFile, 0o444, "The time right now is HH:MM:SS\n".len() as u64),
            NOTES => (FileType::RegularFile, 0o644, self.notes.len() as u64),
            _ => (FileType::RegularFile, 0o444, WEATHER_TEXT.len() as u64),
        };
        FileAttr {
            ino,
            size,
            blocks: 1,
            atime: UNIX_EPOCH,
            mtime: UNIX_EPOCH,
            ctime: UNIX_EPOCH,
            crtime: UNIX_EPOCH,
            kind,
            perm,
            nlink: 1,
            uid: unsafe { libc::getuid() },
            gid: unsafe { libc::getgid() },
            rdev: 0,
            blksize: 512,
            flags: 0,
        }
    }
}

impl Filesystem for MagicFS {
    fn lookup(&mut self, _req: &Request, parent: u64, name: &OsStr, reply: ReplyEntry) {
        let found = FILES.iter().find(|(_, n)| parent == ROOT && name.to_str() == Some(n));
        match found {
            Some(&(ino, name)) => {
                eprintln!("[magicfs] LOOKUP {name} -> ino={ino}");
                reply.entry(&TTL, &self.attr(ino), 0);
            }
            None => reply.error(libc::ENOENT),
        }
    }

    fn getattr(&mut self, _req: &Request, ino: u64, reply: ReplyAttr) {
        reply.attr(&TTL, &self.attr(ino));
    }

    fn open(&mut self, _req: &Request, _ino: u64, _flags: i32, reply: ReplyOpen) {
        reply.opened(0, fuser::consts::FOPEN_DIRECT_IO);
    }

    fn read(
        &mut self,
        _req: &Request,
        ino: u64,
        _fh: u64,
        offset: i64,
        size: u32,
        _flags: i32,
        _lock: Option<u64>,
        reply: ReplyData,
    ) {
        let name = FILES.iter().find(|(i, _)| *i == ino).map(|(_, n)| *n).unwrap_or("?");
        eprintln!("[magicfs] READ {name} (ino={ino}) offset={offset} size={size}");
        let data = self.contents(ino).into_bytes();
        let start = (offset as usize).min(data.len());
        let end = (start + size as usize).min(data.len());
        reply.data(&data[start..end]);
    }

    fn write(&mut self, _req: &Request, ino: u64, _fh: u64, offset: i64, data: &[u8],
             _write_flags: u32, _flags: i32, _lock_owner: Option<u64>, reply: ReplyWrite) {
        if ino != NOTES {
            reply.error(libc::EACCES);
            return;
        }
        eprintln!("[magicfs] WRITE notes.txt (ino={ino}) offset={offset} len={}", data.len());
        let offset = offset as usize;
        let end = offset + data.len();
        if self.notes.len() < end {
            self.notes.resize(end, 0);
        }
        self.notes[offset..end].copy_from_slice(data);
        reply.written(data.len() as u32);
    }

    #[allow(clippy::too_many_arguments)]
    fn setattr(&mut self, _req: &Request, ino: u64, _mode: Option<u32>, _uid: Option<u32>,
               _gid: Option<u32>, size: Option<u64>, _atime: Option<TimeOrNow>,
               _mtime: Option<TimeOrNow>, _ctime: Option<SystemTime>, _fh: Option<u64>,
               _crtime: Option<SystemTime>, _chgtime: Option<SystemTime>,
               _bkuptime: Option<SystemTime>, _flags: Option<u32>, reply: ReplyAttr) {
        if ino == NOTES {
            if let Some(size) = size {
                self.notes.truncate(size as usize);
            }
        }
        reply.attr(&TTL, &self.attr(ino));
    }

    fn readdir(
        &mut self,
        _req: &Request,
        ino: u64,
        _fh: u64,
        offset: i64,
        mut reply: ReplyDirectory,
    ) {
        if ino != ROOT {
            reply.error(libc::ENOTDIR);
            return;
        }
        eprintln!("[magicfs] READDIR ino={ino}");
        let mut entries = vec![(ROOT, FileType::Directory, "."), (ROOT, FileType::Directory, "..")];
        for &(ino, name) in FILES {
            entries.push((ino, FileType::RegularFile, name));
        }
        for (i, (ino, kind, name)) in entries.into_iter().enumerate().skip(offset as usize) {
            if reply.add(ino, (i + 1) as i64, kind, name) {
                break;
            }
        }
        reply.ok();
    }
}

fn main() {
    let mountpoint = std::env::args().nth(1).expect("usage: magicfs <mountpoint>");
    let fs = MagicFS { weather_cached: false, notes: Vec::new() };
    let options = vec![MountOption::FSName("magicfs".into())];
    eprintln!("[magicfs] mounted at {mountpoint}");
    fuser::mount2(fs, &mountpoint, &options).unwrap();
}
