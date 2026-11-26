#!/usr/bin/env bash
# Focused self-tests for scripts/test.sh.
#
# The runner's selection logic is easier to test against a small workspace than
# against this repository: every target can be named, every reverse dependency
# can be checked, and no test code has to compile. The real script is copied
# into a temporary Git checkout, so the tests exercise the same file a
# contributor runs.
set -euo pipefail

SCRIPT_DIR=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
RUNNER="$SCRIPT_DIR/test.sh"
REAL_ROOT=$(cd -- "$SCRIPT_DIR/.." && pwd)
TMP=$(mktemp -d "${TMPDIR:-/tmp}/pdfe-test-runner.XXXXXX")
trap 'rm -rf "$TMP"' EXIT

fail() {
    printf 'not ok - %s\n' "$*" >&2
    exit 1
}

ok() {
    printf 'ok - %s\n' "$*"
}

# Stub cargo binaries below must answer `cargo metadata` the way the real Cargo
# does, while replacing only the test-command calls they are testing.
REAL_CARGO=$(command -v cargo) || {
    printf 'cargo is not available\n' >&2
    exit 1
}
export REAL_CARGO
REAL_PYTHON3=$(command -v python3) || {
    printf 'python3 is not available\n' >&2
    exit 1
}
export REAL_PYTHON3
REAL_BASH=$(command -v bash) || {
    printf 'bash is not available\n' >&2
    exit 1
}

# Resolve the workspace's declared targets once for owner validation. A source
# path is not enough: a custom test target must actually be declared in its
# manifest, so the same metadata lookup the runner uses is the authority here.
declare -A OWNER_TEST_TARGETS=()
declare -A OWNER_LIB_CRATES=()
load_owner_targets() {
    local root=$1 listing kind path name
    OWNER_TEST_TARGETS=()
    OWNER_LIB_CRATES=()
    if ! listing=$(cd "$root" && "$REAL_CARGO" metadata --no-deps --format-version 1 2>/dev/null \
        | "$REAL_PYTHON3" -c '
import json
import os
import sys

root = os.path.realpath(sys.argv[1])
data = json.load(sys.stdin)
for package in data.get("packages", []):
    manifest = package.get("manifest_path")
    if not manifest:
        continue
    package_dir = os.path.realpath(os.path.dirname(manifest))
    for target in package.get("targets", []):
        source = target.get("src_path")
        if not source:
            continue
        kinds = target.get("kind", [])
        if "lib" in kinds or "proc-macro" in kinds:
            print("lib\t%s" % os.path.relpath(package_dir, root))
        if "test" in kinds:
            print(
                "test\t%s\t%s"
                % (os.path.relpath(os.path.realpath(source), root), target.get("name", ""))
            )
' "$root"); then
        printf '%s: could not read cargo metadata for target validation\n' "$root" >&2
        return 1
    fi
    while IFS=$'\t' read -r kind path name; do
        case "$kind" in
            lib) [[ -n $path ]] && OWNER_LIB_CRATES["$path"]=1 ;;
            test) [[ -n $path && -n $name ]] && OWNER_TEST_TARGETS["$path"]=$name ;;
        esac
    done <<<"$listing"
}

# Check a fixture-ownership table: every non-comment row has exactly two
# tab-separated fields, and every owner names either a declared test target
# (including a custom source path outside tests/) or the crate for a `#lib`
# unit-test target.
owner_map_problems() {
    local root=$1 table=$2 line pattern owner crate bad=0 number=0
    local tab=$'\t'
    if ! load_owner_targets "$root"; then
        return 1
    fi
    if [[ ! -f $table ]]; then
        printf '%s: missing fixture-owner table\n' "$table"
        return 1
    fi
    while IFS= read -r line || [[ -n $line ]]; do
        number=$((number + 1))
        line=${line%$'\r'}
        if [[ -z $line || $line == \#* ]]; then
            continue
        fi
        if [[ $line != *"$tab"* ]]; then
            printf 'row %d: missing tab between fixture and owner: %s\n' "$number" "$line"
            bad=1
            continue
        fi
        pattern=${line%%"$tab"*}
        owner=${line#*"$tab"}
        if [[ $owner == *"$tab"* ]]; then
            printf 'row %d: more than two fields: %s\n' "$number" "$line"
            bad=1
            continue
        fi
        if [[ -z $pattern || -z $owner ]]; then
            printf 'row %d: empty fixture or owner: %s\n' "$number" "$line"
            bad=1
            continue
        fi
        if [[ $pattern != test-data/* ]]; then
            printf 'row %d: fixture is not under test-data: %s\n' "$number" "$pattern"
            bad=1
        fi
        if [[ $owner == crates/*'#'lib ]]; then
            crate=${owner#crates/}
            crate=${crate%'#'lib}
            if [[ -z $crate || $crate == */* || ! -f "$root/crates/$crate/Cargo.toml" ]]; then
                printf 'row %d: owner crate is missing: %s\n' "$number" "$owner"
                bad=1
            elif [[ -z ${OWNER_LIB_CRATES["crates/$crate"]:-} ]]; then
                printf 'row %d: owner crate has no lib target: %s\n' "$number" "$owner"
                bad=1
            fi
        elif [[ $owner =~ ^crates/([^/]+)/(.+\.rs)$ ]]; then
            crate=${BASH_REMATCH[1]}
            if [[ ! -f "$root/$owner" ]]; then
                printf 'row %d: owner test target is missing: %s\n' "$number" "$owner"
                bad=1
            elif [[ ! -f "$root/crates/$crate/Cargo.toml" ]]; then
                printf 'row %d: owner crate is missing: %s\n' "$number" "$owner"
                bad=1
            elif [[ -z ${OWNER_TEST_TARGETS["$owner"]:-} ]]; then
                printf 'row %d: owner is not a declared test target: %s\n' "$number" "$owner"
                bad=1
            fi
        else
            printf 'row %d: owner is not a test source or crate#lib: %s\n' "$number" "$owner"
            bad=1
        fi
    done <"$table"
    return "$bad"
}

# Match a fixture path against an ownership row. This mirrors the runner's
# matching, including a bare `dir/*` row covering every descendant and the
# directory itself.
fixture_coverage_matches() {
    local path=$1 pattern=$2 prefix suffix
    [[ $path == $pattern ]] && return 0
    prefix=${pattern%%\**}
    prefix=${prefix%%\?*}
    prefix=${prefix%%\[*}
    [[ $prefix == */ ]] || return 1
    [[ $path == "${prefix%/}" ]] && return 0
    suffix=${pattern#"$prefix"}
    [[ $suffix == '*' && $path == "$prefix"* ]]
}

# A bracket wildcard must be closed. The table uses only shell globs, so an
# unmatched bracket is a typo rather than a literal part of a fixture name.
fixture_coverage_pattern_is_valid() {
    local pattern=$1 brackets=0 index char
    for ((index = 0; index < ${#pattern}; index++)); do
        char=${pattern:index:1}
        case "$char" in
            '[') brackets=$((brackets + 1)) ;;
            ']')
                ((brackets > 0)) || return 1
                brackets=$((brackets - 1))
                ;;
        esac
    done
    ((brackets == 0))
}

# These are generators, an input to a generator, and documentation rather than
# fixtures a test reads. Every other tracked test-data file must have a row.
# A generator is written in Python or in shell (`make_encrypted_unicode.sh`), so
# the rule is its name rather than one extension.
fixture_coverage_excluded() {
    local path=$1
    [[ $path == test-data/*.py || $path == test-data/make_* ]] && return 0
    case "$path" in
        test-data/golden/regenerate.py|test-data/images/stretch.svg|test-data/photos/README.md)
            return 0
            ;;
    esac
    return 1
}

# Check that every tracked fixture has an owner and every owner row points at a
# real fixture. This runs against the repository's actual table as a coverage
# gate, and against the synthetic table below as a test of the report itself.
fixture_owner_coverage_problems() {
    local root=$1 table=$2
    local path pattern owner matched owned problems=0
    local -a files=() patterns=()
    if ! git -C "$root" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
        printf 'coverage: %s is not a Git work tree\n' "$root"
        return 1
    fi
    if [[ ! -f $table ]]; then
        printf 'coverage: missing fixture-owner table: %s\n' "$table"
        return 1
    fi
    # Coverage must not bless a row whose owner is merely a file. When the
    # table is checked against a real Cargo workspace, run the same
    # metadata-backed owner validation before the file-coverage audit.
    if [[ -f "$root/Cargo.toml" ]] \
        && ! owner_map_problems "$root" "$table" >/dev/null 2>&1; then
        printf 'coverage: fixture-owner rows failed target validation\n'
        problems=1
    fi
    while IFS= read -r path; do
        [[ -n $path ]] && files+=("$path")
    done < <(git -C "$root" ls-files -- test-data)
    while IFS=$'\t' read -r pattern owner; do
        [[ -n $pattern && $pattern != \#* ]] || continue
        if [[ -z $owner ]]; then
            printf 'invalid fixture-owner row: %s\n' "$pattern"
            problems=1
            continue
        fi
        if [[ $pattern != test-data/* ]]; then
            printf 'invalid fixture-owner pattern: %s\n' "$pattern"
            problems=1
            continue
        fi
        if ! fixture_coverage_pattern_is_valid "$pattern"; then
            printf 'invalid fixture-owner pattern: %s\n' "$pattern"
            problems=1
            continue
        fi
        patterns+=("$pattern")
        matched=0
        for path in "${files[@]}"; do
            if fixture_coverage_matches "$path" "$pattern"; then
                matched=1
                break
            fi
        done
        if ((matched == 0)); then
            printf 'unmatchable fixture-owner pattern: %s\n' "$pattern"
            problems=1
        fi
    done <"$table"
    for path in "${files[@]}"; do
        fixture_coverage_excluded "$path" && continue
        owned=0
        for pattern in "${patterns[@]}"; do
            if fixture_coverage_matches "$path" "$pattern"; then
                owned=1
                break
            fi
        done
        if ((owned == 0)); then
            printf 'unowned test-data file: %s\n' "$path"
            problems=1
        fi
    done
    return "$problems"
}

if ! owner_map_problems "$REAL_ROOT" "$SCRIPT_DIR/fixture-owners.tsv" >"$TMP/owner-map-problems" 2>&1; then
    cat "$TMP/owner-map-problems" >&2
    fail 'fixture-owner map rows and owners are valid'
fi
ok 'fixture-owner map rows and owners are valid'

if ! fixture_owner_coverage_problems "$REAL_ROOT" "$SCRIPT_DIR/fixture-owners.tsv" >"$TMP/coverage-problems" 2>&1; then
    cat "$TMP/coverage-problems" >&2
    fail 'every tracked test-data fixture has an owner and every pattern matches'
fi
ok 'fixture-owner table covers every tracked test-data fixture'

mkdir -p "$TMP/scripts"
install -m 755 "$RUNNER" "$TMP/scripts/test.sh"
install -m 644 "$SCRIPT_DIR/fixture-owners.tsv" "$TMP/scripts/fixture-owners.tsv"
mkdir -p "$TMP/crates/a/src" "$TMP/crates/a/checks" \
    "$TMP/crates/b/src" "$TMP/crates/b/tests" \
    "$TMP/crates/c/src" "$TMP/crates/cli/src" "$TMP/crates/cli/tests" \
    "$TMP/crates/custom/src" "$TMP/crates/binonly/src" "$TMP/crates/inactive/src"

cat >"$TMP/Cargo.toml" <<'EOF'
[workspace]
members = [
    "crates/a",
    "crates/b",
    "crates/c",
    "crates/cli",
    "crates/custom",
    "crates/binonly",
    "crates/inactive",
]
resolver = "2"

[workspace.dependencies]
renamed_b = { package = "fake-b", path = "crates/b" }
EOF

cat >"$TMP/crates/a/Cargo.toml" <<'EOF'
[package]
name = "fake-a"
version = "0.1.0"
edition = "2021"

[[test]]
name = "custom_named"
path = "checks/custom.rs"
EOF

cat >"$TMP/crates/b/Cargo.toml" <<'EOF'
[package]
name = "fake-b"
version = "0.1.0"
edition = "2021"

[target.'cfg(unix)'.dependencies]
a_alias = { package = "fake-a", path = "../a" }
EOF

cat >"$TMP/crates/c/Cargo.toml" <<'EOF'
[package]
name = "fake-c"
version = "0.1.0"
edition = "2021"

[dependencies]
renamed_b = { workspace = true }
EOF

cat >"$TMP/crates/custom/Cargo.toml" <<'EOF'
[package]
name = "fake-custom"
version = "0.1.0"
edition = "2021"

[lib]
name = "custom_lib"
path = "src/custom_lib.rs"

[[bin]]
name = "custom_bin"
path = "src/custom_bin.rs"
EOF

cat >"$TMP/crates/binonly/Cargo.toml" <<'EOF'
[package]
name = "fake-binonly"
version = "0.1.0"
edition = "2021"

[[bin]]
name = "only_bin"
path = "src/only_bin.rs"
EOF

cat >"$TMP/crates/inactive/Cargo.toml" <<'EOF'
[package]
name = "fake-inactive"
version = "0.1.0"
edition = "2021"

[target.'cfg(target_os = "nonexistent")'.dependencies]
inactive_a = { package = "fake-a", path = "../a" }
EOF

cat >"$TMP/crates/cli/Cargo.toml" <<'EOF'
[package]
name = "pdfextractor-cli"
version = "0.1.0"
edition = "2021"

[dependencies]
fake-c = { path = "../c" }
EOF

# Empty targets are enough: --list reads manifests and target names, not code.
: >"$TMP/crates/a/src/lib.rs"
: >"$TMP/crates/a/checks/custom.rs"
: >"$TMP/crates/a/checks/not_declared.rs"
: >"$TMP/crates/b/src/lib.rs"
: >"$TMP/crates/b/tests/one.rs"
: >"$TMP/crates/b/tests/two.rs"
: >"$TMP/crates/c/src/lib.rs"
: >"$TMP/crates/cli/src/lib.rs"
: >"$TMP/crates/cli/tests/documentation_test.rs"
: >"$TMP/crates/custom/src/custom_lib.rs"
: >"$TMP/crates/custom/src/custom_bin.rs"
: >"$TMP/crates/binonly/src/only_bin.rs"
: >"$TMP/crates/inactive/src/lib.rs"
: >"$TMP/README.md"
mkdir -p "$TMP/test-data" "$TMP/test-data/golden/docling/sub" \
    "$TMP/test-data/real/sub" "$TMP/test-data/pdf-only"
: >"$TMP/test-data/owned.txt"
: >"$TMP/test-data/custom.txt"
: >"$TMP/test-data/unowned.txt"
: >"$TMP/test-data/deleted.txt"
: >"$TMP/test-data/golden/docling/report.pdf"
: >"$TMP/test-data/golden/docling/sub/report.pdf"
: >"$TMP/test-data/real/paper.pdf"
: >"$TMP/test-data/real/sub/paper.pdf"
: >"$TMP/test-data/real/deleted.pdf"
: >"$TMP/test-data/real/notes.txt"
: >"$TMP/test-data/pdf-only/paper.pdf"
: >"$TMP/test-data/pdf-only/notes.txt"
mkdir -p "$TMP/test-data/owned-dir"
: >"$TMP/test-data/owned-dir/input.bin"
# A small, self-contained table: the real table is validated above, while this
# one keeps every synthetic fixture on a known fake target.
cat >"$TMP/scripts/fixture-owners.tsv" <<'EOF'
test-data/owned.txt	crates/b/tests/one.rs
test-data/custom.txt	crates/a/checks/custom.rs
test-data/owned-dir/*	crates/b/tests/two.rs
test-data/golden/docling/*	crates/b/tests/two.rs
test-data/real/*	crates/b/tests/one.rs
test-data/real/*.pdf	crates/b/tests/one.rs
test-data/pdf-only/*.pdf	crates/b/tests/one.rs
test-data/deleted.txt	crates/b/tests/one.rs
EOF

git -C "$TMP" init -q
git -C "$TMP" add -A
git -C "$TMP" -c user.name='test runner' -c user.email='test@example.com' \
    commit -q -m 'fixture workspace'
INITIAL_COMMIT=$(git -C "$TMP" rev-parse HEAD)

run_runner() {
    (cd "$TMP" && "$TMP/scripts/test.sh" "$@")
}

list() {
    run_runner "$@" --list 2>/dev/null
}

reset_fixture() {
    # Reset to the fixture's first commit, not HEAD: the --since scenario below
    # adds a temporary commit that must not leak into later scenarios.
    git -C "$TMP" reset -q --hard "$INITIAL_COMMIT"
    git -C "$TMP" clean -fdq
    rm -rf "$TMP/target"
}

contains() {
    local output=$1 expected=$2 label=$3
    grep -Fq -- "$expected" <<<"$output" || fail "$label: missing: $expected"
}

expect_map_rejected() {
    local label=$1 root=$2 table=$3
    if owner_map_problems "$root" "$table" >/dev/null 2>&1; then
        fail "$label: invalid fixture-owner map was accepted"
    fi
    ok "$label"
}

does_not_contain() {
    local output=$1 unexpected=$2 label=$3
    if grep -Fq -- "$unexpected" <<<"$output"; then
        fail "$label: unexpected: $unexpected"
    fi
}

expect_failure() {
    local label=$1 expected=$2
    shift 2
    local output status
    set +e
    output=$(run_runner "$@" 2>&1)
    status=$?
    set -e
    [[ $status -eq 2 ]] || fail "$label: exit $status, expected 2"
    contains "$output" "$expected" "$label"
}

# Exercise the exact wildcard rows in the shipped table, not just the synthetic
# copies above. Use tracked fixtures so a narrowing or removal of either row
# changes the actual selection. --list resolves metadata and owners but runs no
# test command.
for pattern in 'test-data/golden/docling/*' 'test-data/real/*.pdf'; do
    grep -Fq -- "$pattern"$'\t' "$SCRIPT_DIR/fixture-owners.tsv" \
        || fail "real fixture-owner table lost $pattern"
done
golden_fixture=$(git -C "$REAL_ROOT" ls-files 'test-data/golden/docling/*' | head -n 1)
real_pdf=$(git -C "$REAL_ROOT" ls-files 'test-data/real/*.pdf' | head -n 1)
[[ -n $golden_fixture ]] || fail 'no tracked test-data/golden/docling fixture is available'
[[ -n $real_pdf ]] || fail 'no tracked test-data/real PDF fixture is available'
golden_output=$(cd "$REAL_ROOT" && "$RUNNER" --path "$golden_fixture" --list 2>/dev/null)
pdf_output=$(cd "$REAL_ROOT" && "$RUNNER" --path "$real_pdf" --list 2>/dev/null)
[[ $(printf '%s\n' "$golden_output" | wc -l) -eq 6 ]] \
    || fail "real golden wildcard selected the wrong owner set: $golden_output"
for owner_test in \
    'cargo test -p pdfextractor-cli --test compare_mode_test --locked' \
    'cargo test -p pdfextractor-cli --test docling_golden_test --locked' \
    'cargo test -p pdfextractor-cli --test documentation_test --locked' \
    'cargo test -p pdfextractor-cli --test tracked_fixtures_test --locked' \
    'cargo test -p pdfextractor-ui --test compare_test --locked' \
    'cargo test -p pdfextractor-ui --test corpus_report_test --locked'; do
    contains "$golden_output" "$owner_test" 'real golden wildcard'
done
does_not_contain "$golden_output" 'cargo test --workspace --locked' 'real golden wildcard'
contains "$pdf_output" \
    'cargo test -p pdfextractor-layout --test real_corpus_test --locked' \
    'real PDF wildcard'
does_not_contain "$pdf_output" 'cargo test --workspace --locked' 'real PDF wildcard'
ok 'real table wildcard rows select their actual owners'

# The wildcard rows are path ownership, not a lookup that needs the fixture on
# disk. Export the index (staged and tracked content only) through git archive,
# remove test-data, and select missing paths using the archived runner and
# table. This also proves a clean checkout carries both files.
archive_root="$TMP/clean-archive"
archive_tree=$(git -C "$REAL_ROOT" write-tree) \
    || fail 'could not write the staged tree for the clean archive'
archive_file="$TMP/clean-archive.tar"
git -C "$REAL_ROOT" archive --format=tar "$archive_tree" >"$archive_file" \
    || fail 'could not export the staged tree'
mkdir -p "$archive_root"
tar -xf "$archive_file" -C "$archive_root" \
    || fail 'could not extract the staged tree'
[[ -f "$archive_root/scripts/test.sh" ]] \
    || fail 'clean archive is missing the runner'
[[ -f "$archive_root/scripts/fixture-owners.tsv" ]] \
    || fail 'clean archive is missing the fixture-owner table'
rm -rf "$archive_root/test-data"
[[ ! -e "$archive_root/test-data" ]] \
    || fail 'clean archive still contains test-data fixtures'
golden_archive_output=$(cd "$archive_root" && ./scripts/test.sh \
    --path test-data/golden/docling/absent.json --list 2>/dev/null) \
    || fail 'clean archive golden wildcard selection failed'
pdf_archive_output=$(cd "$archive_root" && ./scripts/test.sh \
    --path test-data/real/absent.pdf --list 2>/dev/null) \
    || fail 'clean archive PDF wildcard selection failed'
[[ $(printf '%s\n' "$golden_archive_output" | wc -l) -eq 6 ]] \
    || fail "clean archive golden wildcard selected the wrong owner set: $golden_archive_output"
for owner_test in \
    'cargo test -p pdfextractor-cli --test compare_mode_test --locked' \
    'cargo test -p pdfextractor-cli --test docling_golden_test --locked' \
    'cargo test -p pdfextractor-cli --test documentation_test --locked' \
    'cargo test -p pdfextractor-cli --test tracked_fixtures_test --locked' \
    'cargo test -p pdfextractor-ui --test compare_test --locked' \
    'cargo test -p pdfextractor-ui --test corpus_report_test --locked'; do
    contains "$golden_archive_output" "$owner_test" 'clean archive golden wildcard'
done
does_not_contain "$golden_archive_output" 'cargo test --workspace --locked' \
    'clean archive golden wildcard'
[[ $(printf '%s\n' "$pdf_archive_output" | wc -l) -eq 3 ]] \
    || fail "clean archive PDF wildcard selected the wrong owner set: $pdf_archive_output"
contains "$pdf_archive_output" \
    'cargo test -p pdfextractor-layout --test real_corpus_test --locked' \
    'clean archive PDF wildcard'
does_not_contain "$pdf_archive_output" 'cargo test --workspace --locked' \
    'clean archive PDF wildcard'
rm -rf "$archive_root" "$archive_file"
ok 'clean staged archive selects wildcard owners without test-data fixtures'

# The coverage audit reports unowned fixtures and bad patterns, while leaving
# the explicit generator, generator-input and documentation exclusions quiet.
coverage_root="$TMP/coverage-root"
coverage_table="$TMP/coverage-owners.tsv"
mkdir -p "$coverage_root/test-data/golden" "$coverage_root/test-data/images" \
    "$coverage_root/test-data/photos"
: >"$coverage_root/test-data/owned.bin"
: >"$coverage_root/test-data/unowned.bin"
: >"$coverage_root/test-data/make_generator.py"
: >"$coverage_root/test-data/make_shell_generator.sh"
: >"$coverage_root/test-data/golden/regenerate.py"
: >"$coverage_root/test-data/images/stretch.svg"
: >"$coverage_root/test-data/photos/README.md"
cat >"$coverage_table" <<'EOF'
test-data/owned.bin	crates/b/tests/one.rs
test-data/missing/*.pdf	crates/b/tests/one.rs
test-data/bad[pattern	crates/b/tests/one.rs
EOF
git -C "$coverage_root" init -q
git -C "$coverage_root" add -A
set +e
coverage_output=$(fixture_owner_coverage_problems "$coverage_root" "$coverage_table" 2>&1)
coverage_status=$?
set -e
[[ $coverage_status -ne 0 ]] || fail 'coverage audit accepted bad fixture ownership'
contains "$coverage_output" 'unowned test-data file: test-data/unowned.bin' 'coverage audit'
contains "$coverage_output" 'unmatchable fixture-owner pattern: test-data/missing/*.pdf' 'coverage audit'
contains "$coverage_output" 'invalid fixture-owner pattern: test-data/bad[pattern' 'coverage audit'
does_not_contain "$coverage_output" 'test-data/make_generator.py' 'coverage exclusions'
does_not_contain "$coverage_output" 'test-data/make_shell_generator.sh' 'coverage exclusions'
does_not_contain "$coverage_output" 'test-data/golden/regenerate.py' 'coverage exclusions'
does_not_contain "$coverage_output" 'test-data/images/stretch.svg' 'coverage exclusions'
does_not_contain "$coverage_output" 'test-data/photos/README.md' 'coverage exclusions'
rm -rf "$coverage_root" "$coverage_table"
ok 'fixture-owner coverage reports bad rows and honors exclusions'

# A tracked source change selects its own crate and every reverse dependency.
# fake-b reaches fake-a through a renamed dependency under a target table; that
# edge must survive the metadata conversion.
reset_fixture
printf '\n// changed\n' >>"$TMP/crates/a/src/lib.rs"
output=$(list)
contains "$output" 'cargo test -p fake-a --lib --locked' 'changed path'
contains "$output" 'cargo test -p fake-b --lib --locked' 'reverse dependency'
contains "$output" 'cargo test -p fake-c --lib --locked' 'transitive reverse dependency'
contains "$output" 'cargo test -p pdfextractor-cli --lib --locked' 'deep reverse dependency'
does_not_contain "$output" 'cargo test --workspace --locked' 'changed path'
ok 'target-specific renamed dependency keeps the reverse closure'

# The same workspace also has a target-specific dependency whose predicate is
# false for this host. It must not create a reverse edge, or a change to fake-a
# would select a package that cargo never builds on this machine.
reset_fixture
printf '\n// changed\n' >>"$TMP/crates/a/src/lib.rs"
output=$(list)
contains "$output" 'cargo test -p fake-b --lib --locked' 'inactive dependency active edge'
does_not_contain "$output" 'cargo test -p fake-inactive' 'inactive dependency'
ok 'inactive target-specific dependencies do not select dependents'

# fake-c reaches fake-b through a renamed workspace dependency. A change to
# fake-b must still pull in that workspace consumer and its own dependents.
reset_fixture
output=$(list --path crates/b/src/lib.rs)
contains "$output" 'cargo test -p fake-c --lib --locked' 'workspace dependency'
contains "$output" 'cargo test -p pdfextractor-cli --lib --locked' 'workspace dependency'
does_not_contain "$output" 'cargo test -p fake-inactive' 'workspace dependency'
ok 'renamed workspace dependencies keep the reverse closure'

# A lib with a custom name and source path must be resolved through metadata,
# and its bin target must be listed from the same metadata.
reset_fixture
output=$(list --path crates/custom/src/custom_lib.rs)
contains "$output" 'cargo test -p fake-custom --lib --locked' 'custom lib target'
contains "$output" 'cargo test -p fake-custom --bins --locked' 'custom bin target'
output=$(run_runner --path crates/custom/src/custom_lib.rs --list --json 2>/dev/null)
python3 - "$output" <<'PY'
import json
import sys

data = json.loads(sys.argv[1])
lib = next(target for target in data["targets"] if target["name"] == "custom_lib")
bin_target = next(target for target in data["targets"] if target["name"] == "custom_bin")
assert lib["package"] == "fake-custom", lib
assert lib["kind"] == "lib", lib
assert lib["source_path"] == "crates/custom/src/custom_lib.rs", lib
assert bin_target["package"] == "fake-custom", bin_target
assert bin_target["kind"] == "bin", bin_target
assert bin_target["source_path"] == "crates/custom/src/custom_bin.rs", bin_target
PY
ok 'custom bin and lib targets are discovered from metadata'

# A package with only a bin target must not grow empty --lib or --doc targets.
reset_fixture
output=$(list --package fake-binonly)
[[ $output == 'cargo test -p fake-binonly --bins --locked' ]] \
    || fail "bin-only package selected the wrong commands: $output"
output=$(run_runner --package fake-binonly --list --json 2>/dev/null)
python3 - "$output" <<'PY'
import json
import sys

data = json.loads(sys.argv[1])
assert len(data["targets"]) == 1, data
bin_target = data["targets"][0]
assert bin_target["package"] == "fake-binonly", bin_target
assert bin_target["kind"] == "bin", bin_target
assert bin_target["name"] == "only_bin", bin_target
assert bin_target["source_path"] == "crates/binonly/src/only_bin.rs", bin_target
PY
ok 'bin-only packages emit only their bin target'

# A test target declared with a custom name and source path must be discovered
# from cargo metadata, not guessed from a file under tests/.
reset_fixture
output=$(list --package fake-a)
contains "$output" 'cargo test -p fake-a --test custom_named --locked' 'custom target discovery'
output=$(list --path crates/a/checks/custom.rs)
[[ $output == 'cargo test -p fake-a --test custom_named --locked' ]] \
    || fail "custom target selected the wrong command: $output"
ok 'custom test target is selected by its metadata path and name'

# A fixture can name a custom target by its source path; the runner resolves
# that path to the declared target name through cargo metadata.
reset_fixture
output=$(list --path test-data/custom.txt)
[[ $output == 'cargo test -p fake-a --test custom_named --locked' ]] \
    || fail "custom-target fixture selected the wrong command: $output"
ok 'fixture owner can name a custom test target'

# Metadata-backed owner validation accepts a custom source path and rejects a
# file that exists but was never declared as a test target. The synthetic table
# still covers every tracked fixture.
reset_fixture
valid_map="$TMP/valid-custom-owner.tsv"
printf 'test-data/custom.txt\tcrates/a/checks/custom.rs\n' >"$valid_map"
if ! owner_map_problems "$TMP" "$valid_map" >/dev/null 2>&1; then
    fail 'valid custom test-target owner was rejected'
fi
bad_map="$TMP/bad-custom-owner.tsv"
printf 'test-data/custom.txt\tcrates/a/checks/not_declared.rs\n' >"$bad_map"
expect_map_rejected 'undeclared custom test-target owner is rejected' "$TMP" "$bad_map"
custom_coverage="$TMP/custom-coverage.tsv"
printf 'test-data/custom.txt\tcrates/a/checks/custom.rs\ntest-data/*\tcrates/a/checks/custom.rs\n' >"$custom_coverage"
if ! fixture_owner_coverage_problems "$TMP" "$custom_coverage" >/dev/null 2>&1; then
    fail 'fixture coverage rejected a custom test-target owner'
fi
bad_coverage="$TMP/bad-custom-coverage.tsv"
printf 'test-data/*\tcrates/a/checks/not_declared.rs\n' >"$bad_coverage"
if fixture_owner_coverage_problems "$TMP" "$bad_coverage" >/dev/null 2>&1; then
    fail 'fixture coverage accepted an undeclared custom test-target owner'
fi
ok 'custom test-target owners are validated and covered'

# JSON list output is the machine-readable form of the same selected commands:
# package, target kind/name, source path, reason and command are all present.
reset_fixture
output=$(run_runner --path crates/a/src/lib.rs --list --json 2>/dev/null)
python3 - "$output" <<'PY'
import json
import sys

data = json.loads(sys.argv[1])
assert data["mode"] == "changed", data
assert data["base"] == "HEAD", data
assert data["changed_paths"] == ["crates/a/src/lib.rs"], data
lib = next(
    target for target in data["targets"]
    if target["package"] == "fake-a" and target["kind"] == "lib"
)
assert lib["name"] == "fake_a", lib
assert lib["source_path"] == "crates/a/src/lib.rs", lib
assert lib["reason"] == "changed path crates/a/src/lib.rs", lib
assert lib["command"] == "cargo test -p fake-a --lib --locked", lib
PY
ok '--list --json describes package targets, source paths and reasons'

# A direct custom target carries its declared target name and exact source path,
# including through --explain, without requiring a selected package record.
reset_fixture
output=$(run_runner --path crates/a/checks/custom.rs --explain --json 2>/dev/null)
python3 - "$output" <<'PY'
import json
import sys

data = json.loads(sys.argv[1])
custom = next(target for target in data["targets"] if target["name"] == "custom_named")
assert custom["package"] == "fake-a", custom
assert custom["kind"] == "test", custom
assert custom["source_path"] == "crates/a/checks/custom.rs", custom
assert custom["reason"] == "changed path crates/a/checks/custom.rs", custom
assert custom["command"] == "cargo test -p fake-a --test custom_named --locked", custom
PY
ok '--explain --json resolves custom target metadata'

# Fixture ownership is a selection reason in the JSON record too.
reset_fixture
output=$(run_runner --path test-data/custom.txt --list --json 2>/dev/null)
python3 - "$output" <<'PY'
import json
import sys

data = json.loads(sys.argv[1])
custom = next(target for target in data["targets"] if target["name"] == "custom_named")
assert custom["reason"] == "fixture test-data/custom.txt (owner crates/a/checks/custom.rs)", custom
assert custom["source_path"] == "crates/a/checks/custom.rs", custom
PY
ok '--list --json includes fixture-owner selection reason'

# The unsharded full command has a single workspace record in JSON mode.
reset_fixture
output=$(run_runner --full --list --json 2>/dev/null)
python3 - "$output" <<'PY'
import json
import sys

data = json.loads(sys.argv[1])
assert data["mode"] == "full", data
assert data["targets"] == [{
    "package": None,
    "kind": "workspace",
    "name": "workspace",
    "source_path": None,
    "reason": "--full",
    "command": "cargo test --workspace --locked",
}], data
PY
ok '--full --list --json emits a workspace record'

# JSON is a view of --list/--explain, not a standalone mode.
expect_failure '--json without a view is rejected' '--json needs --list or --explain' --json

# An untracked integration-test file selects only that target.
reset_fixture
: >"$TMP/crates/b/tests/new_test.rs"
output=$(list)
[[ $(printf '%s\n' "$output" | wc -l) -eq 1 ]] \
    || fail "untracked path: expected one command, got: $output"
[[ $output == 'cargo test -p fake-b --test new_test --locked' ]] \
    || fail "untracked path selected the wrong command: $output"
ok 'untracked changed path selects its integration target'

# --path follows the same reverse closure without touching Git.
reset_fixture
output=$(list --path crates/a/src/lib.rs)
contains "$output" 'cargo test -p fake-a --lib --locked' '--path dependency'
contains "$output" 'cargo test -p fake-b --lib --locked' '--path reverse dependency'
contains "$output" 'cargo test -p fake-c --lib --locked' '--path transitive dependency'
ok '--path selects reverse dependencies'

# --package deliberately does not expand to dependents.
reset_fixture
output=$(list --package fake-a)
contains "$output" 'cargo test -p fake-a --lib --locked' '--package'
does_not_contain "$output" 'cargo test -p fake-b' '--package'
does_not_contain "$output" 'cargo test -p fake-c' '--package'
ok '--package stays inside one crate'

# Cargo test options and filters are part of every selected command, so a
# focused run can stay inside this runner instead of falling back to a hand
# written `cargo test` invocation.
reset_fixture
set +e
output=$(run_runner --package fake-b --features alpha,beta --no-run --no-fail-fast --filter one --list 2>&1)
status=$?
set -e
[[ $status -eq 0 ]] || fail "pass-through list failed with $status: $output"
[[ $(printf '%s\n' "$output" | wc -l) -eq 3 ]] \
    || fail "pass-through options selected the wrong command count: $output"
does_not_contain "$output" 'cargo test -p fake-b --doc' 'pass-through --no-run doc skip'
for target_args in '--lib' '--test one' '--test two'; do
    contains "$output" \
        "cargo test -p fake-b $target_args --locked --features alpha,beta --no-run --no-fail-fast one" \
        'pass-through cargo options'
done
output=$(run_runner --list --package fake-binonly --filter one -- --nocapture --exact 2>/dev/null)
[[ $output == 'cargo test -p fake-binonly --bins --locked one -- --nocapture --exact' ]] \
    || fail "-- did not pass libtest arguments through: $output"
ok 'cargo test options and filters pass through to selected commands'

# The selected execution path must use the same options, not only --list.
reset_fixture
mkdir -p "$TMP/pass-bin"
cat >"$TMP/pass-bin/cargo" <<'EOF'
#!/usr/bin/env bash
if [[ ${1:-} == metadata ]]; then exec "$REAL_CARGO" "$@"; fi
printf '%s\n' "$*" >>"${CARGO_CALLS:?}"
exit 0
EOF
chmod +x "$TMP/pass-bin/cargo"
: >"$TMP/pass-calls"
set +e
pass_output=$(cd "$TMP" && CARGO_CALLS="$TMP/pass-calls" \
    PATH="$TMP/pass-bin:$PATH" "$TMP/scripts/test.sh" --package fake-binonly \
    --features alpha --no-run --no-fail-fast --filter unit_filter -- --nocapture 2>&1)
status=$?
set -e
[[ $status -eq 0 ]] \
    || fail "selected pass-through returned status $status: $pass_output"
[[ $(<"$TMP/pass-calls") == 'test -p fake-binonly --bins --locked --features alpha --no-run --no-fail-fast unit_filter -- --nocapture' ]] \
    || fail "selected pass-through built the wrong command: $(<"$TMP/pass-calls")"
[[ ! -e "$TMP/target/test-runner-timings.tsv" ]] \
    || fail '--no-run recorded a test runtime'
ok 'selected runs execute cargo with the forwarded options'

# Batch mode keeps the selected target set but folds each package's targets
# into one cargo invocation. Cargo accepts repeated target selectors, so the
# grouped command runs neither fewer nor more targets than the normal runner.
reset_fixture
set +e
output=$(run_runner --batch --list --package fake-b 2>&1)
status=$?
set -e
[[ $status -eq 0 ]] || fail "--batch list returned status $status: $output"
[[ $(printf '%s\n' "$output" | wc -l) -eq 2 ]] \
    || fail "--batch did not split docs from other targets: $output"
[[ $output == $'cargo test -p fake-b --doc --locked\ncargo test -p fake-b --lib --test one --test two --locked' ]] \
    || fail "--batch built the wrong grouped commands: $output"
set +e
output=$(run_runner --batch --list --package fake-a --package fake-b 2>&1)
status=$?
set -e
[[ $status -eq 0 ]] || fail "--batch multi-package list returned status $status: $output"
[[ $(printf '%s\n' "$output" | wc -l) -eq 4 ]] \
    || fail "--batch did not keep packages in separate commands: $output"
contains "$output" 'cargo test -p fake-a --doc --locked' '--batch fake-a doc group'
contains "$output" 'cargo test -p fake-a --lib --test custom_named --locked' '--batch fake-a target group'
contains "$output" 'cargo test -p fake-b --doc --locked' '--batch fake-b doc group'
contains "$output" 'cargo test -p fake-b --lib --test one --test two --locked' '--batch fake-b target group'
ok '--batch groups selected targets by package'

# The run path must use the same grouped command as --list, and still forward
# the cargo/libtest options into that one invocation.
reset_fixture
mkdir -p "$TMP/batch-bin"
cat >"$TMP/batch-bin/cargo" <<'EOF'
#!/usr/bin/env bash
if [[ ${1:-} == metadata ]]; then exec "$REAL_CARGO" "$@"; fi
printf '%s\n' "$*" >>"${CARGO_CALLS:?}"
exit 0
EOF
chmod +x "$TMP/batch-bin/cargo"
: >"$TMP/batch-calls"
set +e
batch_output=$(cd "$TMP" && CARGO_CALLS="$TMP/batch-calls" \
    PATH="$TMP/batch-bin:$PATH" "$TMP/scripts/test.sh" --batch --package fake-b \
    --features alpha --no-run --no-fail-fast --filter one -- --nocapture 2>&1)
status=$?
set -e
[[ $status -eq 0 ]] \
    || fail "--batch execution returned status $status: $batch_output"
[[ $(<"$TMP/batch-calls") == 'test -p fake-b --lib --test one --test two --locked --features alpha --no-run --no-fail-fast one -- --nocapture' ]] \
    || fail "--batch execution built the wrong command: $(<"$TMP/batch-calls")"
contains "$batch_output" 'skipping doc target(s)' '--batch no-run doc skip note'
contains "$batch_output" 'batch: 1 cargo invocation(s)' '--batch execution note'
[[ ! -e "$TMP/target/test-runner-timings.tsv" ]] \
    || fail '--batch --no-run recorded a test runtime'
ok '--batch executes one grouped cargo invocation with pass-through options'

# Cargo has no doc-test artifact to build with --no-run. The same rule applies
# to a normal, unbatched package selection.
reset_fixture
set +e
no_run_output=$(run_runner --list --package fake-b --no-run 2>&1)
status=$?
set -e
[[ $status -eq 0 ]] || fail "--no-run list returned status $status: $no_run_output"
[[ $(printf '%s\n' "$no_run_output" | wc -l) -eq 3 ]] \
    || fail "--no-run selected the wrong targets: $no_run_output"
does_not_contain "$no_run_output" 'cargo test -p fake-b --doc' '--no-run doc target'
contains "$no_run_output" 'cargo test -p fake-b --lib --locked --no-run' '--no-run lib target'
ok '--no-run skips doc targets'

# A grouped invocation can fail without identifying which member failed. Keep
# every member in the failure state so --rerun-failed remains target-aware.
reset_fixture
mkdir -p "$TMP/batch-fail-bin"
cat >"$TMP/batch-fail-bin/cargo" <<'EOF'
#!/usr/bin/env bash
if [[ ${1:-} == metadata ]]; then exec "$REAL_CARGO" "$@"; fi
if [[ " $* " == *' --doc '* ]]; then exit 0; fi
exit 3
EOF
chmod +x "$TMP/batch-fail-bin/cargo"
set +e
(cd "$TMP" && PATH="$TMP/batch-fail-bin:$PATH" "$TMP/scripts/test.sh" --batch --package fake-b) >/tmp/batch-fail.out 2>&1
status=$?
set -e
[[ $status -eq 3 ]] || fail "--batch failure returned status $status, expected 3"
[[ $(grep -c '' "$TMP/target/test-runner-failed.tsv") -eq 3 ]] \
    || fail "--batch failure did not record every target in the failed group: $(<"$TMP/target/test-runner-failed.tsv")"
if grep -Fq 'batch|' "$TMP/target/test-runner-failed.tsv"; then
    fail '--batch failure recorded a group instead of its targets'
fi
failed_output=$(run_runner --rerun-failed --list 2>/dev/null)
[[ $(printf '%s\n' "$failed_output" | wc -l) -eq 3 ]] \
    || fail "--rerun-failed did not list each grouped failure: $failed_output"
contains "$failed_output" 'cargo test -p fake-b --test one --locked' '--batch failed target one'
contains "$failed_output" 'cargo test -p fake-b --test two --locked' '--batch failed target two'
ok '--batch failures remain rerunnable as individual targets'

# A Cargo metadata failure must stop selection, not fall back to an empty or
# guessed target list. The stub is the only cargo on PATH and deliberately
# exits nonzero before the runner can parse anything.
reset_fixture
mkdir -p "$TMP/metadata-error-bin"
cat >"$TMP/metadata-error-bin/cargo" <<'EOF'
#!/usr/bin/env bash
if [[ ${1:-} == metadata ]]; then
    exit 9
fi
exit 0
EOF
chmod +x "$TMP/metadata-error-bin/cargo"
set +e
metadata_error_output=$(cd "$TMP" && PATH="$TMP/metadata-error-bin:$PATH" \
    "$TMP/scripts/test.sh" --package fake-b 2>&1)
status=$?
set -e
[[ $status -eq 2 ]] \
    || fail "metadata failure returned status $status, expected 2: $metadata_error_output"
contains "$metadata_error_output" 'cargo metadata could not read the workspace' 'metadata failure'
ok 'cargo metadata errors are fail-closed'

# Metadata that exits successfully but is not JSON must be rejected too; a
# parser traceback must not turn into a successful empty selection.
reset_fixture
mkdir -p "$TMP/metadata-malformed-bin"
cat >"$TMP/metadata-malformed-bin/cargo" <<'EOF'
#!/usr/bin/env bash
if [[ ${1:-} == metadata ]]; then
    printf '{ this is not JSON\n'
    exit 0
fi
exit 0
EOF
chmod +x "$TMP/metadata-malformed-bin/cargo"
set +e
metadata_malformed_output=$(cd "$TMP" && PATH="$TMP/metadata-malformed-bin:$PATH" \
    "$TMP/scripts/test.sh" --package fake-b 2>&1)
status=$?
set -e
[[ $status -eq 2 ]] \
    || fail "malformed metadata returned status $status, expected 2: $metadata_malformed_output"
contains "$metadata_malformed_output" 'cargo metadata could not read the workspace' 'malformed metadata'
ok 'malformed cargo metadata JSON is fail-closed'

# The metadata parser is mandatory. Restrict PATH to a directory containing
# only dirname, then invoke the runner with an absolute bash: the python3 check
# must fail before any target command can run.
reset_fixture
mkdir -p "$TMP/no-python-bin"
ln -sf "$(command -v dirname)" "$TMP/no-python-bin/dirname"
set +e
no_python_output=$(cd "$TMP" && PATH="$TMP/no-python-bin" \
    "$REAL_BASH" "$TMP/scripts/test.sh" --package fake-b 2>&1)
status=$?
set -e
[[ $status -eq 2 ]] \
    || fail "missing python3 returned status $status, expected 2: $no_python_output"
contains "$no_python_output" 'cargo metadata parsing needs python3' 'missing python3'
ok 'missing python3 is fail-closed'

# The unsharded full path is the exact workspace command and must not consult
# metadata. The stub records every cargo invocation, so even a metadata call
# that happens to succeed is visible here.
reset_fixture
mkdir -p "$TMP/full-bin"
cat >"$TMP/full-bin/cargo" <<'EOF'
#!/usr/bin/env bash
printf '%s\n' "$*" >>"${CARGO_CALLS:?}"
printf '%s\n' "${DOCLING_PY-}" >>"${CARGO_CALLS:?}.docling-py"
exit 0
EOF
chmod +x "$TMP/full-bin/cargo"
: >"$TMP/full-calls"
set +e
full_output=$(cd "$TMP" && DOCLING_SKIP_ORACLE=1 CARGO_CALLS="$TMP/full-calls" \
    PATH="$TMP/full-bin:$PATH" "$TMP/scripts/test.sh" --full 2>&1)
status=$?
set -e
[[ $status -eq 0 ]] \
    || fail "full fast path returned status $status: $full_output"
[[ $(grep -c '' "$TMP/full-calls") -eq 1 ]] \
    || fail "full fast path made extra cargo calls: $(<"$TMP/full-calls")"
[[ $(<"$TMP/full-calls") == 'test --workspace --locked' ]] \
    || fail "full fast path did not use the exact workspace command: $(<"$TMP/full-calls")"
contains "$full_output" 'running the explicit full workspace suite' 'full fast path'
ok '--full fast path does not call cargo metadata'

# The unsharded --full fast path must forward the same options instead of
# bypassing them with its one exact workspace command.
: >"$TMP/full-calls"
set +e
full_extra_output=$(cd "$TMP" && DOCLING_SKIP_ORACLE=1 CARGO_CALLS="$TMP/full-calls" \
    PATH="$TMP/full-bin:$PATH" "$TMP/scripts/test.sh" --full \
    --features full --no-run --no-fail-fast --filter filtered -- --nocapture 2>&1)
status=$?
set -e
[[ $status -eq 0 ]] \
    || fail "full pass-through returned status $status: $full_extra_output"
[[ $(<"$TMP/full-calls") == 'test --workspace --locked --features full --no-run --no-fail-fast filtered -- --nocapture' ]] \
    || fail "full pass-through built the wrong command: $(<"$TMP/full-calls")"
contains "$full_extra_output" 'running the explicit full workspace suite' 'full pass-through'

# The release gate resolves the docling oracle itself: the machine's venv when
# the caller named no DOCLING_PY, an early stop with the test's instruction when
# there is neither, and never an opt-out on the caller's behalf. HOME is
# redirected so the answer does not depend on the machine running the tests.
gate_venv="$TMP/gate-home/.venvs/pdfextractor-docling/bin/python"
mkdir -p "$(dirname "$gate_venv")" "$TMP/gate-empty-home"
printf '#!/usr/bin/env bash\nexit 0\n' >"$gate_venv"
chmod +x "$gate_venv"
: >"$TMP/full-calls"
: >"$TMP/full-calls.docling-py"
set +e
gate_venv_output=$(cd "$TMP" && HOME="$TMP/gate-home" CARGO_CALLS="$TMP/full-calls" \
    PATH="$TMP/full-bin:$PATH" "$TMP/scripts/test.sh" --full 2>&1)
status=$?
set -e
[[ $status -eq 0 ]] \
    || fail "machine venv run returned status $status: $gate_venv_output"
[[ $(<"$TMP/full-calls.docling-py") == "$gate_venv" ]] \
    || fail "the machine venv did not become DOCLING_PY: $(<"$TMP/full-calls.docling-py")"
contains "$gate_venv_output" "docling oracle: DOCLING_PY=$gate_venv" 'gate oracle'
ok '--full uses the machine venv when DOCLING_PY is unset'

: >"$TMP/full-calls"
set +e
gate_missing_output=$(cd "$TMP" && HOME="$TMP/gate-empty-home" CARGO_CALLS="$TMP/full-calls" \
    PATH="$TMP/full-bin:$PATH" "$TMP/scripts/test.sh" --full 2>&1)
status=$?
set -e
[[ $status -eq 2 ]] \
    || fail "a run with no oracle returned status $status: $gate_missing_output"
[[ ! -s $TMP/full-calls ]] \
    || fail "cargo ran without an oracle: $(<"$TMP/full-calls")"
contains "$gate_missing_output" 'DOCLING_PY=~/.venvs/pdfextractor-docling/bin/python ./scripts/test.sh --full' 'gate oracle'
contains "$gate_missing_output" 'DOCLING_SKIP_ORACLE=1 ./scripts/test.sh --full' 'gate oracle'
ok '--full stops before cargo when no interpreter can be found'

: >"$TMP/full-calls"
: >"$TMP/full-calls.docling-py"
set +e
gate_named_output=$(cd "$TMP" && HOME="$TMP/gate-empty-home" DOCLING_PY=/opt/python/bin/python \
    CARGO_CALLS="$TMP/full-calls" PATH="$TMP/full-bin:$PATH" "$TMP/scripts/test.sh" --full 2>&1)
status=$?
set -e
[[ $status -eq 0 ]] \
    || fail "a named DOCLING_PY run returned status $status: $gate_named_output"
[[ $(<"$TMP/full-calls.docling-py") == '/opt/python/bin/python' ]] \
    || fail "the caller's DOCLING_PY was overridden: $(<"$TMP/full-calls.docling-py")"
ok "--full leaves the caller's DOCLING_PY alone"

: >"$TMP/full-calls"
: >"$TMP/full-calls.docling-py"
set +e
gate_optout_output=$(cd "$TMP" && HOME="$TMP/gate-empty-home" DOCLING_SKIP_ORACLE=1 \
    CARGO_CALLS="$TMP/full-calls" PATH="$TMP/full-bin:$PATH" "$TMP/scripts/test.sh" --full 2>&1)
status=$?
set -e
[[ $status -eq 0 ]] \
    || fail "an opt-out run returned status $status: $gate_optout_output"
[[ -z $(<"$TMP/full-calls.docling-py") ]] \
    || fail "the runner set DOCLING_PY under an opt-out: $(<"$TMP/full-calls.docling-py")"
ok '--full honours DOCLING_SKIP_ORACLE and sets nothing itself'
ok '--full forwards cargo test options and filters'

# Root documentation selects the CLI's documentation target.
reset_fixture
output=$(list --path README.md)
[[ $output == 'cargo test -p pdfextractor-cli --test documentation_test --locked' ]] \
    || fail "documentation path selected the wrong command: $output"
ok 'README selects the documentation test'

# A change under scripts/ selects the runner's own focused tests, not the
# workspace suite whose selection rules the change could affect.
reset_fixture
: >"$TMP/scripts/helper.sh"
output=$(list)
[[ $output == './scripts/test_runner_test.sh' ]] \
    || fail "scripts change selected the wrong command: $output"
ok 'scripts change selects the runner self-tests'

# A known fixture selects only its declared owner target.
reset_fixture
output=$(list --path test-data/owned.txt)
[[ $output == 'cargo test -p fake-b --test one --locked' ]] \
    || fail "owned fixture selected the wrong command: $output"
ok 'known fixture selects only its owner'

# The map check rejects rows whose owner target or crate has disappeared, and
# malformed rows, instead of letting a stale ownership table pass silently.
bad_map="$TMP/bad-missing-owner.tsv"
printf 'test-data/owned.txt\tcrates/b/tests/missing.rs\n' >"$bad_map"
expect_map_rejected 'missing fixture-owner test target is rejected' "$TMP" "$bad_map"
bad_map="$TMP/bad-missing-crate.tsv"
printf 'test-data/owned.txt\tcrates/not-a-crate#lib\n' >"$bad_map"
expect_map_rejected 'missing fixture-owner crate is rejected' "$TMP" "$bad_map"
bad_map="$TMP/bad-lib-owner.tsv"
printf 'test-data/owned.txt\tcrates/custom/src/custom_lib.rs\n' >"$bad_map"
expect_map_rejected 'non-test metadata target fixture owner is rejected' "$TMP" "$bad_map"
bad_map="$TMP/bad-binonly-lib-owner.tsv"
printf 'test-data/owned.txt\tcrates/binonly#lib\n' >"$bad_map"
expect_map_rejected 'bin-only package fixture-owner lib is rejected' "$TMP" "$bad_map"
bad_map="$TMP/bad-no-separator.tsv"
printf 'test-data/owned.txt crates/b/tests/one.rs\n' >"$bad_map"
expect_map_rejected 'fixture-owner row without a tab is rejected' "$TMP" "$bad_map"
bad_map="$TMP/bad-extra-field.tsv"
printf 'test-data/owned.txt\tcrates/b/tests/one.rs\textra\n' >"$bad_map"
expect_map_rejected 'fixture-owner row with an extra field is rejected' "$TMP" "$bad_map"

# The same ownership applies to a fixture discovered through Git changes.
reset_fixture
printf '\n' >>"$TMP/test-data/owned.txt"
output=$(list)
[[ $output == 'cargo test -p fake-b --test one --locked' ]] \
    || fail "changed fixture selected the wrong command: $output"
ok 'changed fixture selects only its owner'

# --since reads the committed change from the named revision, not just the
# working tree the default HEAD comparison sees.
reset_fixture
printf '\n# committed fixture change\n' >>"$TMP/test-data/owned.txt"
git -C "$TMP" add -- test-data/owned.txt
git -C "$TMP" -c user.name='test runner' -c user.email='test@example.com' \
    commit -q -m 'change an owned fixture'
output=$(list --since HEAD~1)
[[ $output == 'cargo test -p fake-b --test one --locked' ]] \
    || fail "--since selected the wrong command: $output"
ok '--since selects a committed fixture change'

# --since compares from the merge base, not from the upstream tip. An upstream
# commit that the feature branch has not merged must not select its fixture's
# owner, while the feature branch's own fixture change still must.
reset_fixture
git -C "$TMP" branch upstream-merge-base "$INITIAL_COMMIT"
git -C "$TMP" checkout -q upstream-merge-base
printf '\n# upstream-only fixture change\n' >>"$TMP/test-data/owned-dir/input.bin"
git -C "$TMP" add -- test-data/owned-dir/input.bin
git -C "$TMP" -c user.name='test runner' -c user.email='test@example.com' \
    commit -q -m 'advance upstream'
git -C "$TMP" checkout -q -b feature-merge-base "$INITIAL_COMMIT"
printf '\n# feature-only fixture change\n' >>"$TMP/test-data/owned.txt"
git -C "$TMP" add -- test-data/owned.txt
git -C "$TMP" -c user.name='test runner' -c user.email='test@example.com' \
    commit -q -m 'change feature fixture'
raw_changes=$(git -C "$TMP" diff --name-only upstream-merge-base --)
contains "$raw_changes" 'test-data/owned-dir/input.bin' 'upstream diff precondition'
output=$(list --since upstream-merge-base)
[[ $output == 'cargo test -p fake-b --test one --locked' ]] \
    || fail "merge-base --since selected the wrong command: $output"
does_not_contain "$output" 'cargo test -p fake-b --test two --locked' \
    'merge-base --since ignored an upstream-only change'
ok '--since compares from the merge base instead of the upstream tip'

# Unrelated histories have no merge base. That is a selection error, not an
# empty change set, and must fail before cargo is consulted.
reset_fixture
git -C "$TMP" checkout -q --orphan unrelated-base
printf 'unrelated base\n' >"$TMP/unrelated-base.txt"
git -C "$TMP" add unrelated-base.txt
git -C "$TMP" -c user.name='test runner' -c user.email='test@example.com' \
    commit -q -m 'unrelated base'
git -C "$TMP" checkout -q --orphan unrelated-feature
printf 'unrelated feature\n' >"$TMP/unrelated-feature.txt"
git -C "$TMP" add unrelated-feature.txt
git -C "$TMP" -c user.name='test runner' -c user.email='test@example.com' \
    commit -q -m 'unrelated feature'
set +e
output=$(run_runner --since unrelated-base 2>&1)
status=$?
set -e
[[ $status -eq 2 ]] \
    || fail "--since without a merge base: exit $status, expected 2: $output"
contains "$output" '--since unrelated-base has no merge base with HEAD' \
    '--since without a merge base'
git -C "$TMP" checkout -q -B fixture-main "$INITIAL_COMMIT"
ok '--since rejects unrelated histories without a merge base'

# A rename changes both the old and new path. Git's --name-only output would
# otherwise report only the new path for a detected rename, missing the old
# owner's tests; --no-renames makes both paths visible to fixture selection.
reset_fixture
git -C "$TMP" mv test-data/owned.txt test-data/owned-dir/renamed.bin
output=$(list)
[[ $output == $'cargo test -p fake-b --test one --locked\ncargo test -p fake-b --test two --locked' ]] \
    || fail "renamed path selected the wrong commands: $output"
ok 'renamed paths select both the old and new fixture owners'

# The same is true when the rename is in a committed feature change selected
# through --since, not only in the working tree.
reset_fixture
git -C "$TMP" mv test-data/owned.txt test-data/owned-dir/renamed-committed.bin
git -C "$TMP" add -A
git -C "$TMP" -c user.name='test runner' -c user.email='test@example.com' \
    commit -q -m 'rename an owned fixture'
output=$(list --since HEAD~1)
[[ $output == $'cargo test -p fake-b --test one --locked\ncargo test -p fake-b --test two --locked' ]] \
    || fail "--since renamed path selected the wrong commands: $output"
ok '--since selects both owners of a renamed fixture'

# A missing --since revision is refused before any selected target starts. A
# stub cargo records calls, so an accidentally reached test command is visible
# even though the runner itself is the only thing this test runs.
reset_fixture
[[ $(git -C "$TMP" rev-parse HEAD) == "$INITIAL_COMMIT" ]] \
    || fail '--since temporary commit leaked into a later scenario'
mkdir -p "$TMP/since-bin"
cat >"$TMP/since-bin/cargo" <<'EOF'
#!/usr/bin/env bash
if [[ ${1:-} == metadata ]]; then exec "$REAL_CARGO" "$@"; fi
printf '%s\n' "$*" >>"$CARGO_CALLS"
EOF
chmod +x "$TMP/since-bin/cargo"
: >"$TMP/since-calls"
set +e
output=$(cd "$TMP" && CARGO_CALLS="$TMP/since-calls" \
    PATH="$TMP/since-bin:$PATH" "$TMP/scripts/test.sh" --since does-not-exist 2>&1)
status=$?
set -e
[[ $status -eq 2 ]] || fail "--since missing revision: exit $status, expected 2: $output"
contains "$output" '--since does-not-exist is not a revision in this checkout' \
    '--since missing revision'
if [[ -s "$TMP/since-calls" ]]; then
    fail "--since missing revision ran a test command: $(cat "$TMP/since-calls")"
fi
ok '--since rejects a missing revision before running tests'

# A directory fixture matches the owner's directory glob.
reset_fixture
output=$(list --path test-data/owned-dir)
[[ $output == 'cargo test -p fake-b --test two --locked' ]] \
    || fail "fixture directory selected the wrong command: $output"
ok 'fixture directory selects its owner'

# A directory wildcard matches a file below its directory, a nested file, and
# the directory path itself. None may fall back to the full workspace.
reset_fixture
output=$(list --path test-data/golden/docling/report.pdf)
[[ $output == 'cargo test -p fake-b --test two --locked' ]] \
    || fail "wildcard fixture selected the wrong command: $output"
output=$(list --path test-data/golden/docling/sub/report.pdf)
[[ $output == 'cargo test -p fake-b --test two --locked' ]] \
    || fail "nested wildcard fixture selected the wrong command: $output"
output=$(list --path test-data/golden/docling)
[[ $output == 'cargo test -p fake-b --test two --locked' ]] \
    || fail "wildcard directory selected the wrong command: $output"
ok 'directory wildcard matches files, nested files, and its directory'

# The exact example rows must select their owners even when they are the only
# rows in the table. This keeps the golden-directory wildcard and the real PDF
# filter pinned independently of the broader synthetic rows above.
reset_fixture
printf 'test-data/golden/docling/*\t%s\ntest-data/real/*.pdf\t%s\n' \
    'crates/b/tests/two.rs' 'crates/b/tests/one.rs' \
    >"$TMP/scripts/fixture-owners.tsv"
output=$(list --path test-data/golden/docling/missing.pdf)
[[ $output == 'cargo test -p fake-b --test two --locked' ]] \
    || fail "example golden wildcard selected the wrong command: $output"
output=$(list --path test-data/real/missing.pdf)
[[ $output == 'cargo test -p fake-b --test one --locked' ]] \
    || fail "example real PDF wildcard selected the wrong command: $output"
output=$(list --path test-data/real/missing.txt)
contains "$output" 'cargo test -p fake-a --lib --locked' 'example PDF filter'
reset_fixture
ok 'example wildcard rows select their owners without full fallback'

# A bare directory wildcard is broad; an extension-specific row remains a
# filter even when it shares the same directory prefix.
reset_fixture
output=$(list --path test-data/real)
[[ $output == 'cargo test -p fake-b --test one --locked' ]] \
    || fail "directory-wildcard directory selected the wrong command: $output"
output=$(list --path test-data/real/not-pdf.txt)
[[ $output == 'cargo test -p fake-b --test one --locked' ]] \
    || fail "directory wildcard did not cover a non-PDF file: $output"
output=$(list --path test-data/real/sub/paper.pdf)
[[ $output == 'cargo test -p fake-b --test one --locked' ]] \
    || fail "nested directory-wildcard file selected the wrong command: $output"
output=$(list --path test-data/pdf-only/paper.pdf)
[[ $output == 'cargo test -p fake-b --test one --locked' ]] \
    || fail "extension-wildcard fixture selected the wrong command: $output"
output=$(list --path test-data/pdf-only/notes.txt)
contains "$output" 'cargo test -p fake-a --lib --locked' 'non-PDF fixture'
ok 'directory wildcard is broad and extension wildcard remains a filter'

# The directory wildcard is path-based, so a deleted nested fixture is selected
# without the file existing on disk.
reset_fixture
output=$(list --path test-data/real/sub/missing.pdf)
[[ $output == 'cargo test -p fake-b --test one --locked' ]] \
    || fail "deleted nested real fixture selected the wrong command: $output"
output=$(list --path test-data/golden/docling/sub/missing.pdf)
[[ $output == 'cargo test -p fake-b --test two --locked' ]] \
    || fail "deleted nested golden fixture selected the wrong command: $output"
ok 'directory wildcards select deleted nested fixtures'

# A deleted direct fixture has no filesystem entry, but its row still selects
# the target rather than the full workspace.
reset_fixture
rm -f "$TMP/test-data/deleted.txt"
output=$(list --path test-data/deleted.txt)
[[ $output == 'cargo test -p fake-b --test one --locked' ]] \
    || fail "deleted direct fixture selected the wrong command: $output"
ok 'deleted direct fixture selects its owner'

# Git reports a deletion by its old path; a directory wildcard must still
# match it, including a nested path.
reset_fixture
git -C "$TMP" rm -q -- test-data/real/sub/paper.pdf
output=$(list)
[[ $output == 'cargo test -p fake-b --test one --locked' ]] \
    || fail "deleted nested real fixture selected the wrong command: $output"
ok 'deleted nested real fixture selects its owner'

reset_fixture
git -C "$TMP" rm -q -- test-data/golden/docling/sub/report.pdf
output=$(list)
[[ $output == 'cargo test -p fake-b --test two --locked' ]] \
    || fail "deleted nested golden fixture selected the wrong command: $output"
ok 'deleted nested golden fixture selects its owner'

# Top-level deleted fixtures under the same directory rows must still work.
reset_fixture
git -C "$TMP" rm -q -- test-data/real/deleted.pdf
output=$(list)
[[ $output == 'cargo test -p fake-b --test one --locked' ]] \
    || fail "deleted wildcard fixture selected the wrong command: $output"
ok 'deleted wildcard fixture selects only its owner'

reset_fixture
git -C "$TMP" rm -q -- test-data/golden/docling/report.pdf
output=$(list)
[[ $output == 'cargo test -p fake-b --test two --locked' ]] \
    || fail "deleted directory-wildcard fixture selected the wrong command: $output"
ok 'deleted directory-wildcard fixture selects only its owner'

# A fixture with no ownership row keeps the broad fallback.
reset_fixture
output=$(list --path test-data/unknown.bin)
contains "$output" 'cargo test -p fake-a --lib --locked' 'unknown fixture'
contains "$output" 'cargo test -p fake-b --test one --locked' 'unknown fixture'
contains "$output" 'cargo test -p pdfextractor-cli --test documentation_test --locked' 'unknown fixture'
ok 'unknown fixture falls back to the full workspace'

# An existing file with no ownership row is unknown too.
reset_fixture
output=$(list --path test-data/unowned.txt)
contains "$output" 'cargo test -p fake-a --lib --locked' 'unowned fixture'
contains "$output" 'cargo test -p fake-b --test one --locked' 'unowned fixture'
ok 'unowned fixture falls back to the full workspace'

# Shards partition the sorted target list and lose nothing.
reset_fixture
full=$(list --package fake-b)
shard_one=$(list --package fake-b --shard 1/2)
shard_two=$(list --package fake-b --shard 2/2)
[[ -n $shard_one && -n $shard_two ]] || fail 'sharding: one shard is empty'
combined=$(printf '%s\n%s\n' "$shard_one" "$shard_two" | sort)
[[ $combined == "$(printf '%s\n' "$full" | sort)" ]] \
    || fail "sharding: shards do not reconstruct the full target list"
if grep -Fqx -- 'cargo test -p fake-b --doc --locked' <<<"$shard_two"; then
    fail 'sharding: the first and second shards overlap'
fi
contains "$shard_one" 'cargo test -p fake-b --doc --locked' 'shard one'
contains "$shard_one" 'cargo test -p fake-b --test one --locked' 'shard one'
contains "$shard_two" 'cargo test -p fake-b --lib --locked' 'shard two'
contains "$shard_two" 'cargo test -p fake-b --test two --locked' 'shard two'
ok 'shards partition the sorted target list'

# A completed target run records every selected target's elapsed time in a
# fixed three-decimal format. The assertion deliberately checks only that
# format: a loaded machine can make even a short target take longer than any
# duration threshold would predict.
reset_fixture
mkdir -p "$TMP/bin"
cat >"$TMP/bin/cargo" <<'EOF'
#!/usr/bin/env bash
if [[ ${1:-} == metadata ]]; then exec "$REAL_CARGO" "$@"; fi
exit 0
EOF
chmod +x "$TMP/bin/cargo"
(cd "$TMP" && PATH="$TMP/bin:$PATH" "$TMP/scripts/test.sh" --package fake-b >/dev/null 2>&1)
[[ -s "$TMP/target/test-runner-timings.tsv" ]] \
    || fail 'timing recording: no timing file was written'

expected_commands=(
    'cargo test -p fake-b --doc --locked'
    'cargo test -p fake-b --lib --locked'
    'cargo test -p fake-b --test one --locked'
    'cargo test -p fake-b --test two --locked'
)
while IFS=$'\t' read -r timing command || [[ -n $timing ]]; do
    [[ $timing =~ ^[0-9]+\.[0-9]{3}$ ]] \
        || fail "timing recording: expected three decimal places, got $timing for $command"
    match=-1
    for index in "${!expected_commands[@]}"; do
        if [[ $command == "${expected_commands[index]}" ]]; then
            match=$index
            break
        fi
    done
    [[ $match -ge 0 ]] \
        || fail "timing recording: unexpected or duplicate entry: $command"
    unset "expected_commands[$match]"
done <"$TMP/target/test-runner-timings.tsv"
((${#expected_commands[@]} == 0)) \
    || fail "timing recording: missing ${expected_commands[*]}"
ok 'completed targets record their runtime'

# A failing selected target must stop the run and keep its exit status. The
# stub is the only cargo on PATH, so this checks the runner without compiling
# or running a real test target.
reset_fixture
mkdir -p "$TMP/failing-bin"
cat >"$TMP/failing-bin/cargo" <<'EOF'
#!/usr/bin/env bash
if [[ ${1:-} == metadata ]]; then exec "$REAL_CARGO" "$@"; fi
printf '%s\n' "$*" >>"${CARGO_CALLS:?}"
exit "${CARGO_STATUS:?}"
EOF
chmod +x "$TMP/failing-bin/cargo"
: >"$TMP/failing-calls"
set +e
(cd "$TMP" && CARGO_CALLS="$TMP/failing-calls" CARGO_STATUS=23 \
    PATH="$TMP/failing-bin:$PATH" "$TMP/scripts/test.sh" --package fake-b >/dev/null 2>&1)
status=$?
set -e
[[ $status -eq 23 ]] \
    || fail "failing target returned status $status, expected 23"
[[ $(wc -l <"$TMP/failing-calls") -eq 1 ]] \
    || fail "failing target let a later target run: $(<"$TMP/failing-calls")"
[[ $(<"$TMP/failing-calls") == 'test -p fake-b --doc --locked' ]] \
    || fail "the stub cargo was not called with the first selected target"
ok 'a failing selected target stops later targets and returns its status'

# The stop is not special to the first target: make the third selected target
# fail and check that the fourth one is never invoked.
reset_fixture
mkdir -p "$TMP/failing-bin"
cat >"$TMP/failing-bin/cargo" <<'EOF'
#!/usr/bin/env bash
if [[ ${1:-} == metadata ]]; then exec "$REAL_CARGO" "$@"; fi
printf '%s\n' "$*" >>"${CARGO_CALLS:?}"
[[ "$*" == *"${CARGO_FAIL_ON:?}"* ]] && exit "${CARGO_STATUS:?}"
exit 0
EOF
chmod +x "$TMP/failing-bin/cargo"
: >"$TMP/failing-later-calls"
set +e
(cd "$TMP" && CARGO_CALLS="$TMP/failing-later-calls" \
    CARGO_FAIL_ON='--test one' CARGO_STATUS=31 \
    PATH="$TMP/failing-bin:$PATH" "$TMP/scripts/test.sh" --package fake-b >/dev/null 2>&1)
status=$?
set -e
[[ $status -eq 31 ]] \
    || fail "later failing target returned status $status, expected 31"
[[ $(wc -l <"$TMP/failing-later-calls") -eq 3 ]] \
    || fail "later failing target ran the wrong number of targets: $(<"$TMP/failing-later-calls")"
for command in \
    'test -p fake-b --doc --locked' \
    'test -p fake-b --lib --locked' \
    'test -p fake-b --test one --locked'; do
    grep -Fqx -- "$command" "$TMP/failing-later-calls" \
        || fail "later failing target did not run $command"
done
if grep -Fqx -- 'test -p fake-b --test two --locked' "$TMP/failing-later-calls"; then
    fail 'a target after the failing target was run'
fi
ok 'a later failing selected target stops targets after it'

# Parallel mode must actually overlap independent targets, and it must keep
# each target's output as one block in selection order. The barrier makes a
# sequential runner time out waiting for its siblings; the ordered start/end
# lines then distinguish buffered output from interleaved output.
reset_fixture
mkdir -p "$TMP/parallel-bin" "$TMP/parallel-markers"
cat >"$TMP/parallel-bin/cargo" <<'EOF'
#!/usr/bin/env bash
if [[ ${1:-} == metadata ]]; then exec "$REAL_CARGO" "$@"; fi
markers=${PARALLEL_MARKERS:?}
printf 'start %s\n' "$*"
: >"$markers/started.$$"
ready=0
for attempt in $(seq 1 100); do
    count=$(find "$markers" -type f -name 'started.*' | wc -l)
    if ((count >= 4)); then ready=1; break; fi
    sleep 0.01
done
if ((ready == 0)); then
    rm -f "$markers/started.$$"
    printf 'barrier missed\n'
    exit 97
fi
: >"$markers/ready.$$"
ready=0
for attempt in $(seq 1 100); do
    count=$(find "$markers" -type f -name 'ready.*' | wc -l)
    if ((count >= 4)); then ready=1; break; fi
    sleep 0.01
done
if ((ready == 0)); then
    rm -f "$markers/started.$$" "$markers/ready.$$"
    printf 'ready barrier missed\n'
    exit 98
fi
printf 'end %s\n' "$*"
exit 0
EOF
chmod +x "$TMP/parallel-bin/cargo"
set +e
parallel_output=$(cd "$TMP" && PARALLEL_MARKERS="$TMP/parallel-markers" \
    PATH="$TMP/parallel-bin:$PATH" "$TMP/scripts/test.sh" \
    --jobs 4 --package fake-b 2>&1)
parallel_status=$?
set -e
[[ $parallel_status -eq 0 ]] \
    || fail "parallel run failed: $parallel_status: $parallel_output"
actual=$(printf '%s\n' "$parallel_output" | grep -E '^(start|end) ' || true)
expected=$(cat <<'EOF'
start test -p fake-b --doc --locked
end test -p fake-b --doc --locked
start test -p fake-b --lib --locked
end test -p fake-b --lib --locked
start test -p fake-b --test one --locked
end test -p fake-b --test one --locked
start test -p fake-b --test two --locked
end test -p fake-b --test two --locked
EOF
)
[[ $actual == "$expected" ]] \
    || fail "parallel logs were not buffered in selection order: $actual"
ok 'parallel targets overlap and their logs stay in selection order'

# --jobs is a hard concurrency cap, not just a batch size hint. Each fake cargo
# process waits for a sibling before finishing, so a sequential runner times
# out. The shared counter records the peak; the selected target order and the
# failure in the second batch prove that buffering and propagation still work.
reset_fixture
mkdir -p "$TMP/jobs-two-bin"
jobs_state="$TMP/jobs-two-state"
rm -rf "$jobs_state"
mkdir -p "$jobs_state"
cat >"$TMP/jobs-two-bin/cargo" <<'EOF'
#!/usr/bin/env bash
if [[ ${1:-} == metadata ]]; then exec "$REAL_CARGO" "$@"; fi
state=${CARGO_STATE:?}
printf '%s\n' "$*" >>"${CARGO_CALLS:?}"
counted=0
cleanup() {
    if ((counted)); then
        while ! mkdir "$state/lock" 2>/dev/null; do sleep 0.005; done
        active=$(cat "$state/active" 2>/dev/null || printf '0')
        active=$((active - 1))
        if ((active > 0)); then
            printf '%s\n' "$active" >"$state/active"
        else
            rm -f "$state/active"
        fi
        rmdir "$state/lock"
    fi
}
trap cleanup EXIT
while ! mkdir "$state/lock" 2>/dev/null; do sleep 0.005; done
active=$(cat "$state/active" 2>/dev/null || printf '0')
active=$((active + 1))
printf '%s\n' "$active" >"$state/active"
max=$(cat "$state/max" 2>/dev/null || printf '0')
if ((active > max)); then printf '%s\n' "$active" >"$state/max"; fi
rmdir "$state/lock"
counted=1
printf 'start %s\n' "$*"
# Hold the slot long enough for the sibling process to enter. The shared max
# counter, not elapsed time, is what the test asserts.
sleep 0.3
printf 'end %s\n' "$*"
if [[ $* == *'--test one'* ]]; then
    printf 'intentional jobs-two failure\n'
    exit 33
fi
exit 0
EOF
chmod +x "$TMP/jobs-two-bin/cargo"
: >"$TMP/jobs-two-calls"
set +e
jobs_two_output=$(cd "$TMP" && CARGO_STATE="$jobs_state" \
    CARGO_CALLS="$TMP/jobs-two-calls" PATH="$TMP/jobs-two-bin:$PATH" \
    "$TMP/scripts/test.sh" --jobs 2 --package fake-b 2>&1)
jobs_two_status=$?
set -e
[[ $jobs_two_status -eq 33 ]] \
    || fail "jobs-two failure returned status $jobs_two_status, expected 33: $jobs_two_output"
[[ -s "$jobs_state/max" && $(<"$jobs_state/max") -eq 2 ]] \
    || fail "jobs 2 exceeded its concurrency cap: max $(cat "$jobs_state/max" 2>/dev/null)"
[[ $(wc -l <"$TMP/jobs-two-calls") -eq 4 ]] \
    || fail "jobs 2 did not run every selected target: $(<"$TMP/jobs-two-calls")"
actual=$(printf '%s\n' "$jobs_two_output" | grep -E '^(start|end) ' || true)
expected=$(cat <<'EOF'
start test -p fake-b --doc --locked
end test -p fake-b --doc --locked
start test -p fake-b --lib --locked
end test -p fake-b --lib --locked
start test -p fake-b --test one --locked
end test -p fake-b --test one --locked
start test -p fake-b --test two --locked
end test -p fake-b --test two --locked
EOF
)
[[ $actual == "$expected" ]] \
    || fail "jobs 2 logs were not buffered in selection order: $actual"
contains "$jobs_two_output" 'intentional jobs-two failure' 'jobs-two failure log'
contains "$jobs_two_output" 'parallel: 1 target(s) failed' 'jobs-two failure report'
ok '--jobs 2 caps concurrency, preserves ordered logs, and propagates failure'

# A failure in one parallel target must not hide the others' output. The first
# selected target fails, so the sibling in its batch and every later batch must
# still run; the run returns the failing status and names the first failure.
reset_fixture
mkdir -p "$TMP/parallel-fail-bin"
cat >"$TMP/parallel-fail-bin/cargo" <<'EOF'
#!/usr/bin/env bash
if [[ ${1:-} == metadata ]]; then exec "$REAL_CARGO" "$@"; fi
printf '%s\n' "$*" >>"${CARGO_CALLS:?}"
printf 'start %s\n' "$*"
if [[ $* == *'--doc'* ]]; then
    printf 'intentional parallel failure\n'
    exit 31
fi
printf 'end %s\n' "$*"
exit 0
EOF
chmod +x "$TMP/parallel-fail-bin/cargo"
: >"$TMP/parallel-fail-calls"
set +e
parallel_failure_output=$(cd "$TMP" && CARGO_CALLS="$TMP/parallel-fail-calls" \
    PATH="$TMP/parallel-fail-bin:$PATH" "$TMP/scripts/test.sh" \
    --jobs 2 --package fake-b 2>&1)
parallel_failure_status=$?
set -e
[[ $parallel_failure_status -eq 31 ]] \
    || fail "parallel failure returned status $parallel_failure_status, expected 31"
[[ $(wc -l <"$TMP/parallel-fail-calls") -eq 4 ]] \
    || fail "parallel failure stopped a sibling: $(<"$TMP/parallel-fail-calls")"
contains "$parallel_failure_output" 'intentional parallel failure' 'parallel failure log'
contains "$parallel_failure_output" 'parallel: 1 target(s) failed' 'parallel failure report'
ok 'parallel failure runs every target and propagates the failing status'

expect_failure 'zero jobs' 'at least 1' --jobs 0
expect_failure 'non-numeric jobs' 'positive whole number' --jobs nope
expect_failure 'full without shard plus jobs' 'parallelizes selected targets' --full --jobs 2
ok 'invalid parallel options are refused'

# A selected failure records its internal target spec. --rerun-failed reads
# only that spec, and a successful rerun clears the state.
reset_fixture
mkdir -p "$TMP/rerun-fail-bin" "$TMP/rerun-ok-bin"
cat >"$TMP/rerun-fail-bin/cargo" <<'EOF'
#!/usr/bin/env bash
if [[ ${1:-} == metadata ]]; then exec "$REAL_CARGO" "$@"; fi
printf '%s\n' "$*" >>"${CARGO_CALLS:?}"
if [[ $* == *'--test one'* ]]; then exit 31; fi
exit 0
EOF
cat >"$TMP/rerun-ok-bin/cargo" <<'EOF'
#!/usr/bin/env bash
if [[ ${1:-} == metadata ]]; then exec "$REAL_CARGO" "$@"; fi
printf '%s\n' "$*" >>"${CARGO_CALLS:?}"
exit 0
EOF
chmod +x "$TMP/rerun-fail-bin/cargo" "$TMP/rerun-ok-bin/cargo"
: >"$TMP/rerun-fail-calls"
set +e
(cd "$TMP" && CARGO_CALLS="$TMP/rerun-fail-calls" \
    PATH="$TMP/rerun-fail-bin:$PATH" "$TMP/scripts/test.sh" \
    --package fake-b >/dev/null 2>&1)
status=$?
set -e
[[ $status -eq 31 ]] || fail "rerun setup: expected status 31, got $status"
[[ -s "$TMP/target/test-runner-failed.tsv" ]] \
    || fail 'rerun setup: no failed-target state was written'
state_spec="$TMP/crates/b|fake-b|test:one"
grep -Fqx -- "$state_spec" "$TMP/target/test-runner-failed.tsv" \
    || fail "rerun setup: state did not contain $state_spec"
output=$(list --rerun-failed)
[[ $output == 'cargo test -p fake-b --test one --locked' ]] \
    || fail "--rerun-failed selected the wrong command: $output"
: >"$TMP/rerun-ok-calls"
(cd "$TMP" && CARGO_CALLS="$TMP/rerun-ok-calls" \
    PATH="$TMP/rerun-ok-bin:$PATH" "$TMP/scripts/test.sh" --rerun-failed \
    >/dev/null 2>&1) || fail '--rerun-failed did not rerun the recorded target successfully'
[[ ! -e "$TMP/target/test-runner-failed.tsv" ]] \
    || fail '--rerun-failed did not clear the failed-target state after success'
set +e
empty_output=$(cd "$TMP" && "$TMP/scripts/test.sh" --rerun-failed 2>&1)
empty_status=$?
set -e
[[ $empty_status -eq 0 ]] \
    || fail "--rerun-failed without state returned $empty_status"
contains "$empty_output" 'no failed targets recorded' 'empty rerun state'
ok 'failed targets are recorded, rerun alone, and cleared on success'

# A parallel failure can leave more than one failed target. Rerunning them in
# the non-parallel path must visit all of them, not stop after the first and
# discard the rest of the state.
reset_fixture
mkdir -p "$TMP/rerun-many-bin"
cat >"$TMP/rerun-many-bin/cargo" <<'EOF'
#!/usr/bin/env bash
if [[ ${1:-} == metadata ]]; then exec "$REAL_CARGO" "$@"; fi
printf '%s\n' "$*" >>"${CARGO_CALLS:?}"
if [[ $* == *'--test one'* || $* == *'--test two'* ]]; then exit 31; fi
exit 0
EOF
chmod +x "$TMP/rerun-many-bin/cargo"
: >"$TMP/rerun-many-calls"
set +e
(cd "$TMP" && CARGO_CALLS="$TMP/rerun-many-calls" \
    PATH="$TMP/rerun-many-bin:$PATH" "$TMP/scripts/test.sh" \
    --jobs 4 --package fake-b >/dev/null 2>&1)
status=$?
set -e
[[ $status -eq 31 ]] || fail "parallel rerun setup: expected status 31, got $status"
[[ $(wc -l <"$TMP/rerun-many-calls") -eq 4 ]] \
    || fail "parallel rerun setup did not run every target: $(<"$TMP/rerun-many-calls")"
[[ $(wc -l <"$TMP/target/test-runner-failed.tsv") -eq 2 ]] \
    || fail 'parallel rerun setup did not record both failures'
for state_spec in \
    "$TMP/crates/b|fake-b|test:one" \
    "$TMP/crates/b|fake-b|test:two"; do
    grep -Fqx -- "$state_spec" "$TMP/target/test-runner-failed.tsv" \
        || fail "parallel rerun setup lost $state_spec"
done
output=$(list --rerun-failed)
[[ $(printf '%s\n' "$output" | wc -l) -eq 2 ]] \
    || fail "--rerun-failed did not select both failed targets: $output"
contains "$output" 'cargo test -p fake-b --test one --locked' 'multiple rerun one'
contains "$output" 'cargo test -p fake-b --test two --locked' 'multiple rerun two'
: >"$TMP/rerun-many-calls"
set +e
(cd "$TMP" && CARGO_CALLS="$TMP/rerun-many-calls" \
    PATH="$TMP/rerun-many-bin:$PATH" "$TMP/scripts/test.sh" --rerun-failed \
    >/dev/null 2>&1)
status=$?
set -e
[[ $status -eq 31 ]] || fail "multi rerun returned status $status, expected 31"
[[ $(wc -l <"$TMP/rerun-many-calls") -eq 2 ]] \
    || fail "multi rerun stopped after the first failure: $(<"$TMP/rerun-many-calls")"
[[ $(wc -l <"$TMP/target/test-runner-failed.tsv") -eq 2 ]] \
    || fail 'multi rerun dropped the remaining failed targets'
ok 'multiple failed targets are rerun and retained together'

# A normal run updates only the targets it actually ran. Here test two passes
# while the earlier test-one failure is outside this --path selection, so it
# must remain unresolved instead of being cleared by the successful run.
reset_fixture
mkdir -p "$TMP/target" "$TMP/subset-bin"
printf '%s\n%s\n' \
    "$TMP/crates/b|fake-b|test:one" \
    "$TMP/crates/b|fake-b|test:two" \
    >"$TMP/target/test-runner-failed.tsv"
cat >"$TMP/subset-bin/cargo" <<'EOF'
#!/usr/bin/env bash
if [[ ${1:-} == metadata ]]; then exec "$REAL_CARGO" "$@"; fi
printf '%s\n' "$*" >>"${CARGO_CALLS:?}"
exit 0
EOF
chmod +x "$TMP/subset-bin/cargo"
: >"$TMP/subset-calls"
set +e
subset_output=$(cd "$TMP" && CARGO_CALLS="$TMP/subset-calls" \
    PATH="$TMP/subset-bin:$PATH" "$TMP/scripts/test.sh" \
    --path test-data/owned-dir/input.bin 2>&1)
status=$?
set -e
[[ $status -eq 0 ]] \
    || fail "passing subset returned status $status: $subset_output"
[[ $(wc -l <"$TMP/subset-calls") -eq 1 ]] \
    || fail "passing subset ran the wrong targets: $(<"$TMP/subset-calls")"
grep -Fqx -- 'test -p fake-b --test two --locked' "$TMP/subset-calls" \
    || fail "passing subset did not run test two: $(<"$TMP/subset-calls")"
grep -Fqx -- "$TMP/crates/b|fake-b|test:one" "$TMP/target/test-runner-failed.tsv" \
    || fail 'passing subset erased the earlier unrun failure'
if grep -Fqx -- "$TMP/crates/b|fake-b|test:two" "$TMP/target/test-runner-failed.tsv"; then
    fail 'passing subset kept the target that actually passed'
fi
[[ $(wc -l <"$TMP/target/test-runner-failed.tsv") -eq 1 ]] \
    || fail 'passing subset left extra failure state: $(<"$TMP/target/test-runner-failed.tsv")'
ok 'a passing subset preserves failures for targets that did not run'

# Fail-fast stops before later targets. The new doc failure must be recorded,
# while the pre-existing test-two failure is kept because it never ran.
reset_fixture
mkdir -p "$TMP/target" "$TMP/fail-fast-state-bin"
printf '%s\n' "$TMP/crates/b|fake-b|test:two" \
    >"$TMP/target/test-runner-failed.tsv"
cat >"$TMP/fail-fast-state-bin/cargo" <<'EOF'
#!/usr/bin/env bash
if [[ ${1:-} == metadata ]]; then exec "$REAL_CARGO" "$@"; fi
printf '%s\n' "$*" >>"${CARGO_CALLS:?}"
if [[ $* == *'--doc'* ]]; then exit 31; fi
exit 0
EOF
chmod +x "$TMP/fail-fast-state-bin/cargo"
: >"$TMP/fail-fast-state-calls"
set +e
fail_fast_output=$(cd "$TMP" && CARGO_CALLS="$TMP/fail-fast-state-calls" \
    PATH="$TMP/fail-fast-state-bin:$PATH" "$TMP/scripts/test.sh" \
    --package fake-b 2>&1)
status=$?
set -e
[[ $status -eq 31 ]] \
    || fail "fail-fast state run returned status $status: $fail_fast_output"
[[ $(wc -l <"$TMP/fail-fast-state-calls") -eq 1 ]] \
    || fail "fail-fast state run did not stop after doc: $(<"$TMP/fail-fast-state-calls")"
grep -Fqx -- "$TMP/crates/b|fake-b|doc" "$TMP/target/test-runner-failed.tsv" \
    || fail 'fail-fast state did not record the new doc failure'
grep -Fqx -- "$TMP/crates/b|fake-b|test:two" "$TMP/target/test-runner-failed.tsv" \
    || fail 'fail-fast state erased the unrun test-two failure'
if grep -Fqx -- "$TMP/crates/b|fake-b|test:one" "$TMP/target/test-runner-failed.tsv"; then
    fail 'fail-fast state invented a failure for a target that did not run'
fi
ok 'fail-fast records new failures and keeps unrun failures'

# A rerun must remove only the recorded spec that passed. Test one succeeds and
# test two fails, so the rerun leaves exactly test two for the next attempt.
reset_fixture
mkdir -p "$TMP/target" "$TMP/rerun-subset-bin"
printf '%s\n%s\n' \
    "$TMP/crates/b|fake-b|test:one" \
    "$TMP/crates/b|fake-b|test:two" \
    >"$TMP/target/test-runner-failed.tsv"
cat >"$TMP/rerun-subset-bin/cargo" <<'EOF'
#!/usr/bin/env bash
if [[ ${1:-} == metadata ]]; then exec "$REAL_CARGO" "$@"; fi
printf '%s\n' "$*" >>"${CARGO_CALLS:?}"
if [[ $* == *'--test two'* ]]; then exit 31; fi
exit 0
EOF
chmod +x "$TMP/rerun-subset-bin/cargo"
: >"$TMP/rerun-subset-calls"
set +e
rerun_subset_output=$(cd "$TMP" && CARGO_CALLS="$TMP/rerun-subset-calls" \
    PATH="$TMP/rerun-subset-bin:$PATH" "$TMP/scripts/test.sh" \
    --rerun-failed 2>&1)
status=$?
set -e
[[ $status -eq 31 ]] \
    || fail "rerun subset returned status $status: $rerun_subset_output"
[[ $(wc -l <"$TMP/rerun-subset-calls") -eq 2 ]] \
    || fail "rerun subset did not visit both failures: $(<"$TMP/rerun-subset-calls")"
if grep -Fqx -- "$TMP/crates/b|fake-b|test:one" "$TMP/target/test-runner-failed.tsv"; then
    fail 'rerun subset kept the target that passed'
fi
grep -Fqx -- "$TMP/crates/b|fake-b|test:two" "$TMP/target/test-runner-failed.tsv" \
    || fail 'rerun subset erased the unresolved failure'
[[ $(wc -l <"$TMP/target/test-runner-failed.tsv") -eq 1 ]] \
    || fail 'rerun subset left extra failure state: $(<"$TMP/target/test-runner-failed.tsv")'
ok 'a rerun removes only the targets that actually passed'

# Parallel rerun must visit every recorded failure, return the first selected
# failure's status rather than a later sibling's, and keep every unresolved
# failure in the state for the next attempt.
reset_fixture
mkdir -p "$TMP/target" "$TMP/rerun-jobs-bin"
printf '%s\n%s\n' \
    "$TMP/crates/b|fake-b|test:one" \
    "$TMP/crates/b|fake-b|test:two" \
    >"$TMP/target/test-runner-failed.tsv"
cat >"$TMP/rerun-jobs-bin/cargo" <<'EOF'
#!/usr/bin/env bash
if [[ ${1:-} == metadata ]]; then exec "$REAL_CARGO" "$@"; fi
printf '%s\n' "$*" >>"${CARGO_CALLS:?}"
if [[ $* == *'--test one'* ]]; then exit 31; fi
if [[ $* == *'--test two'* ]]; then exit 32; fi
exit 0
EOF
chmod +x "$TMP/rerun-jobs-bin/cargo"
: >"$TMP/rerun-jobs-calls"
set +e
rerun_jobs_output=$(cd "$TMP" && CARGO_CALLS="$TMP/rerun-jobs-calls" \
    PATH="$TMP/rerun-jobs-bin:$PATH" "$TMP/scripts/test.sh" \
    --rerun-failed --jobs 2 2>&1)
status=$?
set -e
[[ $status -eq 31 ]] \
    || fail "parallel rerun returned status $status, expected first failure 31: $rerun_jobs_output"
[[ $(wc -l <"$TMP/rerun-jobs-calls") -eq 2 ]] \
    || fail "parallel rerun did not visit every failure: $(<"$TMP/rerun-jobs-calls")"
grep -Fqx -- 'test -p fake-b --test one --locked' "$TMP/rerun-jobs-calls" \
    || fail 'parallel rerun did not run test one'
grep -Fqx -- 'test -p fake-b --test two --locked' "$TMP/rerun-jobs-calls" \
    || fail 'parallel rerun did not run test two'
[[ $(wc -l <"$TMP/target/test-runner-failed.tsv") -eq 2 ]] \
    || fail "parallel rerun dropped unresolved failures: $(<"$TMP/target/test-runner-failed.tsv")"
for state_spec in \
    "$TMP/crates/b|fake-b|test:one" \
    "$TMP/crates/b|fake-b|test:two"; do
    grep -Fqx -- "$state_spec" "$TMP/target/test-runner-failed.tsv" \
        || fail "parallel rerun lost $state_spec"
done
ok 'parallel rerun visits all failures, returns the first status, and retains the rest'

expect_failure 'rerun with full' 'own mode' --rerun-failed --full
expect_failure 'rerun with package' 'do not combine' --rerun-failed --package fake-b
expect_failure 'rerun with path' 'do not combine' --rerun-failed --path crates/a/src/lib.rs
expect_failure 'rerun with since' 'do not combine' --rerun-failed --since HEAD
expect_failure 'rerun with shard' 'do not combine' --rerun-failed --shard 1/2
ok 'rerun-failed rejects incompatible modes'

# The portable clock fallback must still record a positive sub-second timing
# when Bash's EPOCHREALTIME and GNU date nanoseconds are unavailable. The fake
# clock interpreters make the elapsed value deterministic (250 ms), so this
# does not depend on how loaded the machine is. Both fallback providers are
# exercised: Python first, then Perl after Python fails. Each gets its own
# initialized state file, so the runs cannot leak a clock position into one
# another.
reset_fixture
mkdir -p "$TMP/clock-bin"
printf 'unset EPOCHREALTIME\n' >"$TMP/clock-bin/no-epoch"
cat >"$TMP/clock-bin/python3" <<'EOF'
#!/usr/bin/env bash
if [[ ${1:-} == -c && ${2:-} == *"import json"* ]]; then
    exec "$REAL_PYTHON3" "$@"
fi
runtime=${0##*/}
[[ ${CLOCK_PROVIDER:-} == "$runtime" ]] || exit 1
state=${CLOCK_STATE:?}
value=$(<"$state")
value=$((value + 250))
printf '%s\n' "$value" >"$state"
printf '%s\n' "$value"
EOF
cp "$TMP/clock-bin/python3" "$TMP/clock-bin/perl"
cat >"$TMP/clock-bin/date" <<'EOF'
#!/usr/bin/env bash
[[ ${1:-} == +%s ]] && { printf '1700000000\n'; exit 0; }
exit 1
EOF
cat >"$TMP/clock-bin/cargo" <<'EOF'
#!/usr/bin/env bash
if [[ ${1:-} == metadata ]]; then exec "$REAL_CARGO" "$@"; fi
exit 0
EOF
chmod +x "$TMP/clock-bin/python3" "$TMP/clock-bin/perl" \
    "$TMP/clock-bin/date" "$TMP/clock-bin/cargo"

run_portable_clock() {
    local runtime=$1 entries=0 state="$TMP/clock-state-$1"
    rm -rf "$TMP/target"
    printf '1000\n' >"$state"
    (cd "$TMP" && unset EPOCHREALTIME \
        && BASH_ENV="$TMP/clock-bin/no-epoch" \
            CLOCK_PROVIDER="$runtime" CLOCK_STATE="$state" \
            PATH="$TMP/clock-bin:$PATH" \
            "$TMP/scripts/test.sh" --package fake-b >/dev/null 2>&1) \
        || fail "portable clock: $runtime fallback run failed"
    while IFS=$'\t' read -r timing command || [[ -n $timing ]]; do
        entries=$((entries + 1))
        [[ $timing == '0.250' ]] \
            || fail "portable clock: $runtime recorded $timing for $command, expected 0.250"
    done <"$TMP/target/test-runner-timings.tsv"
    ((entries == 4)) \
        || fail "portable clock: $runtime recorded $entries timings, expected four"
}

run_portable_clock python3
run_portable_clock perl
ok 'portable clock fallback records positive sub-second timings through Python and Perl'

# A sub-second recording is still a recording, so it takes part in the
# duration-aware split instead of falling back to a count-based split.
reset_fixture
mkdir -p "$TMP/target"
cat >"$TMP/target/test-runner-timings.tsv" <<'EOF'
0.100	cargo test -p fake-b --doc --locked
0.100	cargo test -p fake-b --lib --locked
0.100	cargo test -p fake-b --test one --locked
0.900	cargo test -p fake-b --test two --locked
EOF
shard_one=$(list --package fake-b --shard 1/2)
shard_two=$(list --package fake-b --shard 2/2)
contains "$shard_one" 'cargo test -p fake-b --test two --locked' 'subsecond shard one'
does_not_contain "$shard_one" 'cargo test -p fake-b --test one --locked' 'subsecond shard one'
contains "$shard_two" 'cargo test -p fake-b --doc --locked' 'subsecond shard two'
contains "$shard_two" 'cargo test -p fake-b --lib --locked' 'subsecond shard two'
contains "$shard_two" 'cargo test -p fake-b --test one --locked' 'subsecond shard two'
does_not_contain "$shard_two" 'cargo test -p fake-b --test two --locked' 'subsecond shard two'
ok 'sub-second runtimes drive duration-aware sharding'

# Recorded runtimes rebalance the same targets away from the count-based split.
reset_fixture
mkdir -p "$TMP/target"
cat >"$TMP/target/test-runner-timings.tsv" <<'EOF'
10	cargo test -p fake-b --test one --locked
9	cargo test -p fake-b --test two --locked
8	cargo test -p fake-b --doc --locked
1	cargo test -p fake-b --lib --locked
EOF
shard_one=$(list --package fake-b --shard 1/2)
shard_two=$(list --package fake-b --shard 2/2)
contains "$shard_one" 'cargo test -p fake-b --test one --locked' 'timed shard one'
contains "$shard_one" 'cargo test -p fake-b --lib --locked' 'timed shard one'
does_not_contain "$shard_one" 'cargo test -p fake-b --doc --locked' 'timed shard one'
does_not_contain "$shard_one" 'cargo test -p fake-b --test two --locked' 'timed shard one'
contains "$shard_two" 'cargo test -p fake-b --test two --locked' 'timed shard two'
contains "$shard_two" 'cargo test -p fake-b --doc --locked' 'timed shard two'
does_not_contain "$shard_two" 'cargo test -p fake-b --test one --locked' 'timed shard two'
ok 'recorded runtimes balance shards by duration'

# A target with no recording is estimated from the median of the recorded
# targets. This differs from the count split: the 5s recordings steer the two
# 5s targets apart, while the missing test-two target gets the 5s median.
reset_fixture
mkdir -p "$TMP/target"
cat >"$TMP/target/test-runner-timings.tsv" <<'EOF'
5	cargo test -p fake-b --lib --locked
5	cargo test -p fake-b --test one --locked
1	cargo test -p fake-b --doc --locked
EOF
shard_one=$(list --package fake-b --shard 1/2)
shard_two=$(list --package fake-b --shard 2/2)
contains "$shard_one" 'cargo test -p fake-b --lib --locked' 'partial timing'
contains "$shard_one" 'cargo test -p fake-b --test two --locked' 'partial timing'
does_not_contain "$shard_one" 'cargo test -p fake-b --doc --locked' 'partial timing'
does_not_contain "$shard_one" 'cargo test -p fake-b --test one --locked' 'partial timing'
contains "$shard_two" 'cargo test -p fake-b --doc --locked' 'partial timing'
contains "$shard_two" 'cargo test -p fake-b --test one --locked' 'partial timing'
does_not_contain "$shard_two" 'cargo test -p fake-b --lib --locked' 'partial timing'
does_not_contain "$shard_two" 'cargo test -p fake-b --test two --locked' 'partial timing'
ok 'known timings balance shards when a target has no recording'

# With an even number of recorded targets, the fallback is the average of the
# two middle values. The two missing targets must use that 5s median rather
# than a count split or a zero cost.
reset_fixture
mkdir -p "$TMP/target"
cat >"$TMP/target/test-runner-timings.tsv" <<'EOF'
2	cargo test -p fake-b --doc --locked
8	cargo test -p fake-b --lib --locked
EOF
shard_one=$(list --package fake-b --shard 1/2)
shard_two=$(list --package fake-b --shard 2/2)
contains "$shard_one" 'cargo test -p fake-b --lib --locked' 'even partial timing'
contains "$shard_one" 'cargo test -p fake-b --doc --locked' 'even partial timing'
does_not_contain "$shard_one" 'cargo test -p fake-b --test one --locked' 'even partial timing'
does_not_contain "$shard_one" 'cargo test -p fake-b --test two --locked' 'even partial timing'
contains "$shard_two" 'cargo test -p fake-b --test one --locked' 'even partial timing'
contains "$shard_two" 'cargo test -p fake-b --test two --locked' 'even partial timing'
does_not_contain "$shard_two" 'cargo test -p fake-b --lib --locked' 'even partial timing'
does_not_contain "$shard_two" 'cargo test -p fake-b --doc --locked' 'even partial timing'
ok 'even-count partial timings use the median fallback'

# With four unrecorded targets and only two recordings, the median estimate
# must still spread the work across three shards instead of collapsing to a
# count split. Timed targets are sorted by descending duration: the 12s target
# anchors shard one, the four 11.5s median targets fill the other slots, and
# the 11s target returns to shard one. Every shard totals 23s and holds two
# targets.
reset_fixture
: >"$TMP/crates/b/tests/three.rs"
: >"$TMP/crates/b/tests/four.rs"
mkdir -p "$TMP/target"
cat >"$TMP/target/test-runner-timings.tsv" <<'EOF'
12	cargo test -p fake-b --test one --locked
11	cargo test -p fake-b --test two --locked
EOF
shard_one=$(list --package fake-b --shard 1/3)
shard_two=$(list --package fake-b --shard 2/3)
shard_three=$(list --package fake-b --shard 3/3)
[[ $(printf '%s\n' "$shard_one" | wc -l) -eq 2 ]] \
    || fail "three-shard partial timing: shard one is not balanced: $shard_one"
[[ $(printf '%s\n' "$shard_two" | wc -l) -eq 2 ]] \
    || fail "three-shard partial timing: shard two is not balanced: $shard_two"
[[ $(printf '%s\n' "$shard_three" | wc -l) -eq 2 ]] \
    || fail "three-shard partial timing: shard three is not balanced: $shard_three"
contains "$shard_one" 'cargo test -p fake-b --test one --locked' 'three-shard partial timing'
contains "$shard_one" 'cargo test -p fake-b --test two --locked' 'three-shard partial timing'
contains "$shard_two" 'cargo test -p fake-b --doc --locked' 'three-shard partial timing'
contains "$shard_two" 'cargo test -p fake-b --test four --locked' 'three-shard partial timing'
contains "$shard_three" 'cargo test -p fake-b --lib --locked' 'three-shard partial timing'
contains "$shard_three" 'cargo test -p fake-b --test three --locked' 'three-shard partial timing'
ok 'known runtimes balance three shards with multiple unrecorded targets'

# Malformed timing rows are ignored, not treated as zero or as valid timings.
# The valid recordings still steer the split; the malformed doc row becomes the
# median estimate, just like any other target with no valid recording.
reset_fixture
mkdir -p "$TMP/target"
cat >"$TMP/target/test-runner-timings.tsv" <<'EOF'
not-a-number	cargo test -p fake-b --doc --locked
0.100
0.100	cargo test -p fake-b --lib --locked
0.100	cargo test -p fake-b --test one --locked
0.900	cargo test -p fake-b --test two --locked
EOF
shard_one=$(list --package fake-b --shard 1/2)
shard_two=$(list --package fake-b --shard 2/2)
contains "$shard_one" 'cargo test -p fake-b --test two --locked' 'malformed timing'
does_not_contain "$shard_one" 'cargo test -p fake-b --doc --locked' 'malformed timing'
does_not_contain "$shard_one" 'cargo test -p fake-b --lib --locked' 'malformed timing'
does_not_contain "$shard_one" 'cargo test -p fake-b --test one --locked' 'malformed timing'
contains "$shard_two" 'cargo test -p fake-b --doc --locked' 'malformed timing'
contains "$shard_two" 'cargo test -p fake-b --lib --locked' 'malformed timing'
contains "$shard_two" 'cargo test -p fake-b --test one --locked' 'malformed timing'
does_not_contain "$shard_two" 'cargo test -p fake-b --test two --locked' 'malformed timing'
ok 'malformed timing rows are ignored while known timings balance shards'

# A timing file with no usable rows must not disable selection or divide by a
# malformed value. It falls back to the same count-based split as an empty
# timing table, including rows with extra columns and no command.
reset_fixture
mkdir -p "$TMP/target"
cat >"$TMP/target/test-runner-timings.tsv" <<'EOF'
-1	cargo test -p fake-b --doc --locked
NaN	cargo test -p fake-b --lib --locked
.5	cargo test -p fake-b --test one --locked
1.2.3	cargo test -p fake-b --test two --locked
0.100
0.100	cargo test -p fake-b --lib --locked	extra
not-a-number
EOF
shard_one=$(list --package fake-b --shard 1/2)
shard_two=$(list --package fake-b --shard 2/2)
contains "$shard_one" 'cargo test -p fake-b --doc --locked' 'all-malformed timing'
contains "$shard_one" 'cargo test -p fake-b --test one --locked' 'all-malformed timing'
does_not_contain "$shard_one" 'cargo test -p fake-b --lib --locked' 'all-malformed timing'
does_not_contain "$shard_one" 'cargo test -p fake-b --test two --locked' 'all-malformed timing'
contains "$shard_two" 'cargo test -p fake-b --lib --locked' 'all-malformed timing'
contains "$shard_two" 'cargo test -p fake-b --test two --locked' 'all-malformed timing'
does_not_contain "$shard_two" 'cargo test -p fake-b --doc --locked' 'all-malformed timing'
does_not_contain "$shard_two" 'cargo test -p fake-b --test one --locked' 'all-malformed timing'
ok 'a timing file with only malformed rows uses the count split'

# Duplicate rows are last-wins for a target, and malformed duplicates do not
# erase the last valid recording. The last value changes the duration-aware
# assignment away from the count-based split, so this also proves the table was
# actually used rather than merely accepted.
reset_fixture
mkdir -p "$TMP/target"
cat >"$TMP/target/test-runner-timings.tsv" <<'EOF'
0.100	cargo test -p fake-b --doc --locked
0.100	cargo test -p fake-b --lib --locked
0.100	cargo test -p fake-b --test one --locked
not-a-number	cargo test -p fake-b --test one --locked
0.100	cargo test -p fake-b --test two --locked
0.900	cargo test -p fake-b --test one --locked
EOF
shard_one=$(list --package fake-b --shard 1/2)
shard_two=$(list --package fake-b --shard 2/2)
[[ $(printf '%s\n' "$shard_one" | wc -l) -eq 1 ]] \
    || fail "duplicate timing: expected one target in shard one: $shard_one"
contains "$shard_one" 'cargo test -p fake-b --test one --locked' 'duplicate timing'
does_not_contain "$shard_one" 'cargo test -p fake-b --test two --locked' 'duplicate timing'
contains "$shard_two" 'cargo test -p fake-b --doc --locked' 'duplicate timing'
contains "$shard_two" 'cargo test -p fake-b --lib --locked' 'duplicate timing'
contains "$shard_two" 'cargo test -p fake-b --test two --locked' 'duplicate timing'
ok 'duplicate timing rows use the last valid value and stay duration-aware'

# Invalid combinations fail before a test starts.
expect_failure 'zero shard count' 'greater than zero' --shard 1/0
expect_failure 'shard index outside range' 'outside 1..1' --shard 2/1
expect_failure 'malformed shard' 'must be I/N' --shard nope
expect_failure 'features without value' '--features needs a feature list' --features
expect_failure 'filter without value' '--filter needs a test name' --filter
expect_failure 'batch plus json' '--batch groups commands' --batch --list --json
expect_failure 'batch plus full' '--full is already one cargo invocation' --batch --full
expect_failure 'full plus package' 'selects a fast run' --full --package fake-a
expect_failure 'unknown option' 'unknown option' --not-a-real-option
ok 'invalid arguments are refused'

printf 'all test-runner self-tests passed\n'
