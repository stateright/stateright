#/bin/bash

set -e
set -u

COUNT=${1:-3}

bench() {
    echo "== $@ =="
    for i in $(seq $COUNT); do
        cargo -q run --release --example "$@" |grep "sec="
    done
}

bench 2pc check 9
bench paxos check 15
bench single-copy-register check 4
bench linearizable-register check 4
