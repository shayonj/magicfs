#!/bin/sh
set -e
magicfs /magic &
sleep 0.5
echo
echo "magicfs is mounted at /magic. Try:"
echo "  ls /magic"
echo "  cat /magic/time.txt        (twice)"
echo "  time cat /magic/weather.txt   (twice)"
echo "  echo remember the milk > /magic/notes.txt"
echo
if [ "$#" -gt 0 ]; then
    exec "$@"
fi
exec bash
