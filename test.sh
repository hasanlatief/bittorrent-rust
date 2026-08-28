#!/usr/bin/env bash
SLUGS=("ns2" "eb4" "ah1" "mn6" "ow9" "rb2" "bf7" "fi9" "ca4" "nd2" "jv8" "hw0" "pk2" "xi4" "jk6" "ns5" "zh1" "qv6" "dv7")
TESTER_BIN="${TESTER_BIN:-./tester}"
export CODECRAFTERS_REPOSITORY_DIR="/home/hasan/Documents/programming/personal/code_crafters/codecrafters-bittorrent-rust/"
export CODECRAFTERS_SUBMISSION_DIR="$CODECRAFTERS_REPOSITORY_DIR"

for slug in "${SLUGS[@]}"; do
    export CODECRAFTERS_TEST_CASES_JSON="[{\"slug\":\"$slug\",\"tester_log_prefix\":\"$slug\",\"title\":\"$slug\"}]"

    printf "\n===> Running test: %s\n" "$slug"
    if ! "$TESTER_BIN"; then
        printf "\nStage failed: %s\n" "$slug"
        exit 1
    fi
done
printf "\nAll stages passed successfully!\n"