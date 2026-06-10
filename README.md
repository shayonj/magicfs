# magicfs

A tiny FUSE filesystem in Rust where none of the files are real. Code for the post [Building a Tiny Filesystem with FUSE](https://www.shayon.dev/post/2026/161/building-a-tiny-filesystem-with-fuse/).

It mounts a folder with four files: `hello.txt` is made up by the program, `time.txt` changes on every read, `weather.txt` pretends to download itself on first read, and `notes.txt` is writable but only into the program's memory.

## Run it

```
docker run -it --rm --device /dev/fuse --cap-add SYS_ADMIN shayonj/magicfs
```

This drops you into a shell with the filesystem mounted at `/magic`.

## Build it yourself (Linux)

```
sudo apt install fuse3 libfuse3-dev pkg-config
cargo build --release
mkdir /tmp/magic
./target/release/magicfs /tmp/magic
```

Then `ls` and `cat` files under `/tmp/magic` from another terminal. Unmount with `fusermount3 -u /tmp/magic`.
