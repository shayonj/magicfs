#!/bin/sh
set -e
STORE_DIR=/tmp/magicfs-store
magicfs /magic "$STORE_DIR" &
sleep 0.5
echo
echo "magicfs is mounted at /magic and backed by $STORE_DIR. Try:"
echo "  ls /magic"
echo "  cat /magic/hello.txt"
echo "  echo remember the milk > /magic/notes.txt"
echo "  cat /magic/notes.txt"
echo "  cat $STORE_DIR/metadata.json"
echo "  find $STORE_DIR/objects -type f"
echo
if [ "$#" -gt 0 ]; then
    exec "$@"
fi
exec bash
