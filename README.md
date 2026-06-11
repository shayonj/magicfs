# magicfs

A small FUSE filesystem in Rust with metadata in JSON and file contents in local blob files. Code for the post [Building a Tiny FUSE Filesystem](https://www.shayon.dev/post/2026/161/building-a-tiny-fuse-filesystem/).

`magicfs` mounts a directory where normal tools like `ls`, `cat`, `echo`, `mv`, and `rm` work, while the backing store is just metadata plus local blob files named by allocated IDs:

```text
/tmp/magicfs-store/
  metadata.json
  blobs/
    7f/7f83...
```

## Run it

```console
docker run -it --rm --device /dev/fuse --cap-add SYS_ADMIN shayonj/magicfs
```

This drops you into a shell with the filesystem mounted at `/magic` and the backing store at `/tmp/magicfs-store`.

## Build it yourself (Linux)

```console
sudo apt install fuse3
cargo build --release
mkdir -p /tmp/magic
./target/release/magicfs /tmp/magic /tmp/magicfs-store
```

Then run commands against `/tmp/magic` from another terminal. Unmount with `fusermount3 -u /tmp/magic`.

## What to watch

The program logs the FUSE requests it receives. Writes are staged in memory first:

```console
echo "remember the milk" > /magic/notes.txt
```

On `FLUSH` or `FSYNC`, `magicfs` writes a new blob and atomically replaces `metadata.json`. Inspect the store after writing:

```console
cat /tmp/magicfs-store/metadata.json
find /tmp/magicfs-store/blobs -type f
```

## What it skips

`magicfs` only has a single root directory, stores each file as one local blob, and uses a one second metadata TTL. It does not implement a journal, multi-client cache invalidation, locking, mmap, xattrs, a full permission model, chunking, background garbage collection, remote backing stores, or recovery for orphaned blobs.
