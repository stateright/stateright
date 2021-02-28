#/bin/bash

set -e
set -u

COUNT=${1:-3}
FILTER=${2:-''}

echo "Benchmarking with:" >&2
echo "- COUNT=$COUNT"     >&2
echo "- FILTER='$FILTER'" >&2
echo                      >&2

# USAGE: bench EXAMPLE ARGS...
# EXAMPLE: bench 2pc check 9
# 
# No-op if EXAMPLE does not match FILTER.
bench() {
    if [[ $1 == *"$FILTER"* ]]; then
        echo "== $@ =="
        for i in $(seq $COUNT); do
            cargo -q run --release --example "$@" |grep "sec="
        done
    fi
}

bench 2pc check 10
bench paxos check 15
bench single-copy-register check 4
bench linearizable-register check 4
