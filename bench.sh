bench() {
    echo "== $@ =="
    for i in $(seq 3); do
        cargo -q run --release --example "$@" |grep "sec="
    done
}

bench 2pc check 9
bench paxos check 6 2
bench single-copy-register check 3 3
