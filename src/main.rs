use fuser::{
    FileAttr, FileType, Filesystem, MountOption, ReplyAttr, ReplyCreate, ReplyData, ReplyDirectory,
    ReplyEmpty, ReplyEntry, ReplyOpen, ReplyWrite, Request, TimeOrNow,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::ffi::OsStr;
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const TTL: Duration = Duration::from_secs(1);
const ROOT: u64 = 1;
const DEFAULT_FILE_MODE: u16 = 0o644;
const HELLO_TEXT: &[u8] = b"Hello from a tiny FUSE filesystem.\n";

#[derive(Clone, Debug, Serialize, Deserialize)]
struct BlobRef {
    blob: String,
    offset: u64,
    len: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Entry {
    ino: u64,
    mode: u16,
    size: u64,
    blobs: Vec<BlobRef>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Metadata {
    next_inode: u64,
    next_blob: u64,
    entries: BTreeMap<String, Entry>,
}

struct MagicFS {
    store_dir: PathBuf,
    blobs_dir: PathBuf,
    metadata_path: PathBuf,
    meta: Metadata,
    staged: HashMap<u64, Vec<u8>>,
}

impl MagicFS {
    fn open(store_dir: PathBuf) -> io::Result<Self> {
        let blobs_dir = store_dir.join("blobs");
        let metadata_path = store_dir.join("metadata.json");
        fs::create_dir_all(&blobs_dir)?;

        let meta = if metadata_path.exists() {
            let bytes = fs::read(&metadata_path)?;
            serde_json::from_slice(&bytes).map_err(|err| {
                io::Error::new(io::ErrorKind::InvalidData, format!("read metadata: {err}"))
            })?
        } else {
            let mut fs = Self {
                store_dir,
                blobs_dir,
                metadata_path,
                meta: Metadata {
                    next_inode: 2,
                    next_blob: 1,
                    entries: BTreeMap::new(),
                },
                staged: HashMap::new(),
            };
            fs.bootstrap()?;
            fs.commit_metadata()?;
            return Ok(fs);
        };

        Ok(Self {
            store_dir,
            blobs_dir,
            metadata_path,
            meta,
            staged: HashMap::new(),
        })
    }

    fn bootstrap(&mut self) -> io::Result<()> {
        let blob = self.put_blob(HELLO_TEXT)?;
        let ino = self.allocate_inode();
        self.meta.entries.insert(
            "hello.txt".to_string(),
            Entry {
                ino,
                mode: DEFAULT_FILE_MODE,
                size: HELLO_TEXT.len() as u64,
                blobs: vec![BlobRef {
                    blob,
                    offset: 0,
                    len: HELLO_TEXT.len() as u64,
                }],
            },
        );

        let ino = self.allocate_inode();
        self.meta.entries.insert(
            "notes.txt".to_string(),
            Entry {
                ino,
                mode: DEFAULT_FILE_MODE,
                size: 0,
                blobs: Vec::new(),
            },
        );
        Ok(())
    }

    fn allocate_inode(&mut self) -> u64 {
        let ino = self.meta.next_inode;
        self.meta.next_inode += 1;
        ino
    }

    fn allocate_blob(&mut self) -> String {
        let id = format!("blob-{:012}", self.meta.next_blob);
        self.meta.next_blob += 1;
        id
    }

    fn name_for_inode(&self, ino: u64) -> Option<&str> {
        if ino == ROOT {
            return Some(".");
        }
        self.meta
            .entries
            .iter()
            .find(|(_, entry)| entry.ino == ino)
            .map(|(name, _)| name.as_str())
    }

    fn entry_for_inode(&self, ino: u64) -> Option<(&str, &Entry)> {
        self.meta
            .entries
            .iter()
            .find(|(_, entry)| entry.ino == ino)
            .map(|(name, entry)| (name.as_str(), entry))
    }

    fn entry_for_inode_mut(&mut self, ino: u64) -> Option<(&str, &mut Entry)> {
        self.meta
            .entries
            .iter_mut()
            .find(|(_, entry)| entry.ino == ino)
            .map(|(name, entry)| (name.as_str(), entry))
    }

    fn attr_for_entry(&self, entry: &Entry) -> FileAttr {
        FileAttr {
            ino: entry.ino,
            size: entry.size,
            blocks: entry.size.div_ceil(512),
            atime: UNIX_EPOCH,
            mtime: UNIX_EPOCH,
            ctime: UNIX_EPOCH,
            crtime: UNIX_EPOCH,
            kind: FileType::RegularFile,
            perm: entry.mode,
            nlink: 1,
            uid: unsafe { libc::getuid() },
            gid: unsafe { libc::getgid() },
            rdev: 0,
            blksize: 4096,
            flags: 0,
        }
    }

    fn root_attr() -> FileAttr {
        FileAttr {
            ino: ROOT,
            size: 0,
            blocks: 0,
            atime: UNIX_EPOCH,
            mtime: UNIX_EPOCH,
            ctime: UNIX_EPOCH,
            crtime: UNIX_EPOCH,
            kind: FileType::Directory,
            perm: 0o755,
            nlink: 2,
            uid: unsafe { libc::getuid() },
            gid: unsafe { libc::getgid() },
            rdev: 0,
            blksize: 4096,
            flags: 0,
        }
    }

    fn blob_path(&self, blob: &str) -> PathBuf {
        self.blobs_dir.join(blob)
    }

    fn put_blob(&mut self, data: &[u8]) -> io::Result<String> {
        let blob = self.allocate_blob();
        let path = self.blob_path(&blob);
        let tmp = path.with_extension("tmp");
        {
            let mut file = File::create(&tmp)?;
            file.write_all(data)?;
            file.sync_all()?;
        }
        fs::rename(&tmp, &path)?;
        sync_dir(&self.blobs_dir)?;
        Ok(blob)
    }

    fn read_committed(&self, entry: &Entry) -> io::Result<Vec<u8>> {
        let mut data = vec![0; entry.size as usize];
        for object in &entry.blobs {
            let bytes = fs::read(self.blob_path(&object.blob))?;
            let start = object.offset as usize;
            let end = start + object.len as usize;
            data[start..end].copy_from_slice(&bytes[..object.len as usize]);
        }
        Ok(data)
    }

    fn read_inode(&self, ino: u64) -> io::Result<Vec<u8>> {
        if let Some(data) = self.staged.get(&ino) {
            return Ok(data.clone());
        }
        let (_, entry) = self
            .entry_for_inode(ino)
            .ok_or_else(|| io::Error::from_raw_os_error(libc::ENOENT))?;
        self.read_committed(entry)
    }

    fn stage_inode(&mut self, ino: u64) -> io::Result<&mut Vec<u8>> {
        if !self.staged.contains_key(&ino) {
            let data = self.read_inode(ino)?;
            self.staged.insert(ino, data);
        }
        Ok(self.staged.get_mut(&ino).expect("staged entry exists"))
    }

    fn commit_inode(&mut self, ino: u64) -> io::Result<()> {
        let Some(data) = self.staged.remove(&ino) else {
            return Ok(());
        };

        let object = if data.is_empty() {
            Vec::new()
        } else {
            let blob = self.put_blob(&data)?;
            vec![BlobRef {
                blob,
                offset: 0,
                len: data.len() as u64,
            }]
        };

        let (name, entry) = self
            .entry_for_inode_mut(ino)
            .ok_or_else(|| io::Error::from_raw_os_error(libc::ENOENT))?;
        entry.size = data.len() as u64;
        entry.blobs = object;
        eprintln!(
            "[magicfs] COMMIT {name} ino={ino} size={} blobs={}",
            entry.size,
            entry.blobs.len()
        );
        self.commit_metadata()
    }

    fn commit_metadata(&self) -> io::Result<()> {
        fs::create_dir_all(&self.store_dir)?;
        let tmp = self.metadata_path.with_extension("json.tmp");
        {
            let mut file = File::create(&tmp)?;
            serde_json::to_writer_pretty(&mut file, &self.meta).map_err(|err| {
                io::Error::new(io::ErrorKind::Other, format!("write metadata: {err}"))
            })?;
            file.write_all(b"\n")?;
            file.sync_all()?;
        }
        fs::rename(&tmp, &self.metadata_path)?;
        sync_dir(&self.store_dir)?;
        eprintln!(
            "[magicfs] COMMIT metadata entries={}",
            self.meta.entries.len()
        );
        Ok(())
    }
}

fn sync_dir(path: &Path) -> io::Result<()> {
    File::open(path)?.sync_all()
}

impl Filesystem for MagicFS {
    fn lookup(&mut self, _req: &Request, parent: u64, name: &OsStr, reply: ReplyEntry) {
        if parent != ROOT {
            reply.error(libc::ENOENT);
            return;
        }

        let Some(name) = name.to_str() else {
            reply.error(libc::ENOENT);
            return;
        };
        match self.meta.entries.get(name) {
            Some(entry) => {
                eprintln!("[magicfs] LOOKUP {name} -> ino={}", entry.ino);
                reply.entry(&TTL, &self.attr_for_entry(entry), 0);
            }
            None => reply.error(libc::ENOENT),
        }
    }

    fn getattr(&mut self, _req: &Request, ino: u64, reply: ReplyAttr) {
        if ino == ROOT {
            reply.attr(&TTL, &Self::root_attr());
            return;
        }

        match self.entry_for_inode(ino) {
            Some((_, entry)) => reply.attr(&TTL, &self.attr_for_entry(entry)),
            None => reply.error(libc::ENOENT),
        }
    }

    fn open(&mut self, _req: &Request, ino: u64, flags: i32, reply: ReplyOpen) {
        let name = self.name_for_inode(ino).unwrap_or("?");
        eprintln!("[magicfs] OPEN {name} ino={ino} flags=0x{flags:x}");
        if ino == ROOT || self.entry_for_inode(ino).is_none() {
            reply.error(libc::ENOENT);
            return;
        }
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
        let name = self.name_for_inode(ino).unwrap_or("?");
        eprintln!("[magicfs] READ {name} ino={ino} offset={offset} size={size}");
        match self.read_inode(ino) {
            Ok(data) => {
                let start = (offset.max(0) as usize).min(data.len());
                let end = (start + size as usize).min(data.len());
                reply.data(&data[start..end]);
            }
            Err(err) => reply.error(err.raw_os_error().unwrap_or(libc::EIO)),
        }
    }

    fn write(
        &mut self,
        _req: &Request,
        ino: u64,
        _fh: u64,
        offset: i64,
        data: &[u8],
        _write_flags: u32,
        _flags: i32,
        _lock_owner: Option<u64>,
        reply: ReplyWrite,
    ) {
        let name = self.name_for_inode(ino).unwrap_or("?");
        eprintln!(
            "[magicfs] WRITE {name} ino={ino} offset={offset} len={} staged=true",
            data.len()
        );

        let offset = offset.max(0) as usize;
        match self.stage_inode(ino) {
            Ok(staged) => {
                let end = offset + data.len();
                if staged.len() < end {
                    staged.resize(end, 0);
                }
                staged[offset..end].copy_from_slice(data);
                reply.written(data.len() as u32);
            }
            Err(err) => reply.error(err.raw_os_error().unwrap_or(libc::EIO)),
        }
    }

    fn flush(&mut self, _req: &Request, ino: u64, _fh: u64, _lock_owner: u64, reply: ReplyEmpty) {
        let name = self.name_for_inode(ino).unwrap_or("?");
        eprintln!("[magicfs] FLUSH {name} ino={ino}");
        match self.commit_inode(ino) {
            Ok(()) => reply.ok(),
            Err(err) => reply.error(err.raw_os_error().unwrap_or(libc::EIO)),
        }
    }

    fn fsync(&mut self, _req: &Request, ino: u64, _fh: u64, datasync: bool, reply: ReplyEmpty) {
        let name = self.name_for_inode(ino).unwrap_or("?");
        eprintln!("[magicfs] FSYNC {name} ino={ino} datasync={datasync}");
        match self.commit_inode(ino) {
            Ok(()) => reply.ok(),
            Err(err) => reply.error(err.raw_os_error().unwrap_or(libc::EIO)),
        }
    }

    fn release(
        &mut self,
        _req: &Request,
        ino: u64,
        _fh: u64,
        flags: i32,
        _lock_owner: Option<u64>,
        flush: bool,
        reply: ReplyEmpty,
    ) {
        let name = self.name_for_inode(ino).unwrap_or("?");
        eprintln!("[magicfs] RELEASE {name} ino={ino} flags=0x{flags:x} flush={flush}");
        reply.ok();
    }

    #[allow(clippy::too_many_arguments)]
    fn setattr(
        &mut self,
        _req: &Request,
        ino: u64,
        _mode: Option<u32>,
        _uid: Option<u32>,
        _gid: Option<u32>,
        size: Option<u64>,
        _atime: Option<TimeOrNow>,
        _mtime: Option<TimeOrNow>,
        _ctime: Option<SystemTime>,
        _fh: Option<u64>,
        _crtime: Option<SystemTime>,
        _chgtime: Option<SystemTime>,
        _bkuptime: Option<SystemTime>,
        _flags: Option<u32>,
        reply: ReplyAttr,
    ) {
        if let Some(size) = size {
            match self.stage_inode(ino) {
                Ok(staged) => staged.resize(size as usize, 0),
                Err(err) => {
                    reply.error(err.raw_os_error().unwrap_or(libc::EIO));
                    return;
                }
            }
            eprintln!("[magicfs] SETATTR ino={ino} size={size} staged=true");
        }

        match self.entry_for_inode(ino) {
            Some((_, entry)) => {
                let mut attr = self.attr_for_entry(entry);
                if let Some(staged) = self.staged.get(&ino) {
                    attr.size = staged.len() as u64;
                    attr.blocks = attr.size.div_ceil(512);
                }
                reply.attr(&TTL, &attr);
            }
            None => reply.error(libc::ENOENT),
        }
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
        let mut entries = vec![
            (ROOT, FileType::Directory, ".".to_string()),
            (ROOT, FileType::Directory, "..".to_string()),
        ];
        for (name, entry) in &self.meta.entries {
            entries.push((entry.ino, FileType::RegularFile, name.clone()));
        }

        for (i, (ino, kind, name)) in entries.into_iter().enumerate().skip(offset as usize) {
            if reply.add(ino, (i + 1) as i64, kind, name.as_str()) {
                break;
            }
        }
        reply.ok();
    }

    fn create(
        &mut self,
        _req: &Request,
        parent: u64,
        name: &OsStr,
        mode: u32,
        _umask: u32,
        flags: i32,
        reply: ReplyCreate,
    ) {
        if parent != ROOT {
            reply.error(libc::ENOENT);
            return;
        }
        let Some(name) = name.to_str() else {
            reply.error(libc::EINVAL);
            return;
        };
        if self.meta.entries.contains_key(name) {
            reply.error(libc::EEXIST);
            return;
        }

        let ino = self.allocate_inode();
        self.meta.entries.insert(
            name.to_string(),
            Entry {
                ino,
                mode: (mode & 0o777) as u16,
                size: 0,
                blobs: Vec::new(),
            },
        );
        eprintln!("[magicfs] CREATE {name} ino={ino} flags=0x{flags:x}");
        if let Err(err) = self.commit_metadata() {
            reply.error(err.raw_os_error().unwrap_or(libc::EIO));
            return;
        }

        let attr = self.attr_for_entry(self.meta.entries.get(name).expect("created entry exists"));
        reply.created(&TTL, &attr, 0, 0, fuser::consts::FOPEN_DIRECT_IO);
    }

    fn rename(
        &mut self,
        _req: &Request,
        parent: u64,
        name: &OsStr,
        newparent: u64,
        newname: &OsStr,
        flags: u32,
        reply: ReplyEmpty,
    ) {
        if parent != ROOT || newparent != ROOT || flags != 0 {
            reply.error(libc::EINVAL);
            return;
        }
        let Some(name) = name.to_str() else {
            reply.error(libc::EINVAL);
            return;
        };
        let Some(newname) = newname.to_str() else {
            reply.error(libc::EINVAL);
            return;
        };

        match self.meta.entries.remove(name) {
            Some(entry) => {
                eprintln!("[magicfs] RENAME {name} -> {newname} ino={}", entry.ino);
                self.meta.entries.insert(newname.to_string(), entry);
                match self.commit_metadata() {
                    Ok(()) => reply.ok(),
                    Err(err) => reply.error(err.raw_os_error().unwrap_or(libc::EIO)),
                }
            }
            None => reply.error(libc::ENOENT),
        }
    }

    fn unlink(&mut self, _req: &Request, parent: u64, name: &OsStr, reply: ReplyEmpty) {
        if parent != ROOT {
            reply.error(libc::ENOENT);
            return;
        }
        let Some(name) = name.to_str() else {
            reply.error(libc::EINVAL);
            return;
        };

        match self.meta.entries.remove(name) {
            Some(entry) => {
                self.staged.remove(&entry.ino);
                eprintln!(
                    "[magicfs] UNLINK {name} ino={} blobs_left_for_gc={}",
                    entry.ino,
                    entry.blobs.len()
                );
                match self.commit_metadata() {
                    Ok(()) => reply.ok(),
                    Err(err) => reply.error(err.raw_os_error().unwrap_or(libc::EIO)),
                }
            }
            None => reply.error(libc::ENOENT),
        }
    }

    fn forget(&mut self, _req: &Request, ino: u64, nlookup: u64) {
        eprintln!("[magicfs] FORGET ino={ino} nlookup={nlookup}");
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let mountpoint = args
        .next()
        .expect("usage: magicfs <mountpoint> [store-dir]");
    let store_dir = args
        .next()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp/magicfs-store"));

    let fs = MagicFS::open(store_dir.clone()).expect("open magicfs store");
    let options = vec![MountOption::FSName("magicfs".into())];
    eprintln!(
        "[magicfs] mounted at {mountpoint}, backing store at {}",
        store_dir.display()
    );
    fuser::mount2(fs, &mountpoint, &options).unwrap();
}
