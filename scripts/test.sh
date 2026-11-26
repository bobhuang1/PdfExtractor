#!/usr/bin/env bash
# Fast local test selection and sharding.
#
# The workspace suite is the release gate: it runs every test target in every
# crate and takes tens of minutes. Day-to-day work does not need all of it. This
# script reads the files a change touched, walks the workspace's path
# dependencies backwards, and runs only the crates those files can affect — or
# splits the selected targets into a shard for parallel terminals.
#
# The full suite is always explicit:
#
#     scripts/test.sh --full
#
# That is `cargo test --workspace --locked`, unchanged. No GitHub Actions are
# dispatched or modified by this script.
set -eo pipefail

ROOT=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
cd "$ROOT"

usage() {
    cat <<'EOF'
Usage: scripts/test.sh [options]

Fast local verification. By default, files changed against --since (or HEAD)
select the crates and targets that can observe them through the workspace's
path dependencies. Selected targets run with `cargo test --locked`.

Modes:
  --changed             Select from changed files (the default).
  --full, --all         Run the explicit full workspace suite:
                        cargo test --workspace --locked
                        With --shard, split every target instead. The suite's
                        docling_core_accepts_our_documents fails rather than
                        passing silently when nothing can load docling-core:
                        set DOCLING_PY to a Python with docling-core, or opt
                        out in so many words with DOCLING_SKIP_ORACLE=1 for a
                        run that checks the schema alone. A full run uses
                        ~/.venvs/pdfextractor-docling/bin/python when that
                        interpreter exists, and otherwise stops before
                        compiling with the same two options.
  --rerun-failed        Run only the target specs recorded as failed by the
                        previous selected run. Passing targets are removed from
                        target/test-runner-failed.tsv; failures not run remain.
                        This mode is incompatible with --full, --since, --path,
                        --package and --shard.
  --self-test           Run the focused tests for this runner and exit.

Selection:
  --since <REV>         Base revision for --changed (default: HEAD). The
                        merge base of REV and HEAD is compared, so changes
                        only on REV (for example upstream) are not selected.
                        Includes committed, staged, unstaged and untracked
                        changes.
  --path <FILE>         Select as if FILE changed (repeatable), instead of
                        reading Git's changed paths. Useful from an editor.
  --package, -p <NAME>  Run one package's targets (repeatable). No reverse
                        dependency expansion; use --changed for that.
  --features <LIST>     Pass --features LIST to every cargo test.
  --no-run              Pass --no-run: build the selected tests without running.
                        Doc targets are skipped because Cargo cannot combine
                        --doc with --no-run.
  --no-fail-fast        Pass --no-fail-fast to every cargo test.
  --filter <NAME>       Pass a test-name filter before any `--` arguments.
  -- <ARGS>             Pass the remaining arguments directly to cargo test.
  --batch                Group selected targets by package into one cargo test
                         invocation per group. Docs are a separate group because
                         Cargo cannot mix --doc with other target selectors. Each
                         group selects only the requested targets. Not available
                         with an unsharded --full or --json.

Sharding:
  --shard <I/N>         Run shard I of N, with 1 <= I <= N. Balance by
                        recorded target runtime (sub-second values kept) from
                        target/test-runner-timings.tsv. A target with no
                        recording is estimated from the median recorded time;
                        only when no selected target has a recording does it
                        fall back to sorted round-robin by target count. Each
                        target is its own cargo invocation, so feature
                        unification can differ from --full; the unsharded
                        --full command is the release gate.

Parallelism:
  --jobs, -j <N>        Run up to N independent selected targets at once.
                        Defaults to 1. Target output is buffered and printed
                        in selection order, never interleaved. A failure does
                        not cancel the other selected targets; the first
                        failing target's status is returned. --full without
                        --shard is one workspace command, so it cannot be
                        parallelized this way.

Output:
  --list                Print the commands a run would execute and exit.
  --explain             Say which files and crates selected the run.
  --json                With --list or --explain, print a machine-readable JSON
                        object: changed paths and one record per selected target
                        with package, kind, name, source path, reason and command.
  -h, --help            Show this help.

Examples:
  ./scripts/test.sh                         # affected crates only
  ./scripts/test.sh --since origin/main    # changed on this branch
  ./scripts/test.sh --path crates/limits/src/lib.rs
  ./scripts/test.sh --package pdfextractor-limits
  ./scripts/test.sh --full                  # release gate
  ./scripts/test.sh --full --shard 1/4      # one shard, for parallel terminals
  ./scripts/test.sh --jobs 4                # selected targets in parallel
  ./scripts/test.sh --rerun-failed          # retry the last failed targets
  ./scripts/test.sh --list                  # inspect, do not run
  ./scripts/test.sh --list --json            # same, as JSON
  ./scripts/test.sh --self-test             # test the runner itself

A root manifest, Cargo.lock or an unknown path selects the full suite; a change
under scripts/ selects the test-runner self-tests. README/plan/docs select the
CLI documentation test; a top-level integration-test file selects that target.
A test-data fixture selects the targets listed for it in
scripts/fixture-owners.tsv, or the full suite when it has no owner. A `dir/*`
row matches every descendant and the directory itself; an extension row such as
`dir/*.pdf` remains a filter. A deleted fixture still matches the row its old
path named.
EOF
}

die() {
    printf 'test: %s\n' "$*" >&2
    exit 2
}

# The options after the target flags are the same cargo test arguments for
# display and for execution. Keep them in arrays so feature lists, filters and
# `--`-passed libtest arguments survive spaces and shell metacharacters.
display_arg() {
    local arg=$1
    if [[ $arg == *[!A-Za-z0-9_./:=+@%^,-]* ]]; then
        printf '%q' "$arg"
    else
        printf '%s' "$arg"
    fi
}

cargo_extra_command_line() {
    local arg
    for arg in "${CARGO_TEST_OPTIONS[@]}" "${TEST_FILTERS[@]}"; do
        printf ' '
        display_arg "$arg"
    done
    if ((${#CARGO_TEST_PASSTHROUGH[@]} > 0)); then
        printf ' --'
        for arg in "${CARGO_TEST_PASSTHROUGH[@]}"; do
            printf ' '
            display_arg "$arg"
        done
    fi
}

workspace_command_line() {
    printf 'cargo test --workspace --locked'
    cargo_extra_command_line
    printf '\n'
}

# The unsharded --full path deliberately avoids cargo metadata. Its JSON form
# is the one workspace command, described in the same record shape as selected
# targets from the metadata path.
emit_workspace_json() {
    local command
    command=$(workspace_command_line)
    command -v python3 >/dev/null 2>&1 \
        || die "JSON output needs python3"
    python3 -c '
import json
import sys

print(json.dumps({
    "mode": "full",
    "base": sys.argv[1],
    "changed_paths": [],
    "targets": [{
        "package": None,
        "kind": "workspace",
        "name": "workspace",
        "source_path": None,
        "reason": "--full",
        "command": sys.argv[2],
    }],
}, indent=2))
' "$BASE" "$command"
}

note() {
    printf 'test: %s\n' "$*" >&2
}

# The docling oracle: the interpreter `docling_core_accepts_our_documents` loads
# docling-core with. The test fails, by design, when it has none, because a run
# that cannot load the library must not report the corpus as accepted. The
# release gate resolves it here rather than leaving it to whoever starts the
# run: the machine's venv when the caller named no `DOCLING_PY`, and an early
# stop with the test's own instruction when neither exists. `DOCLING_SKIP_ORACLE`
# is only ever honoured, never set: opting out is the caller's decision to make
# in so many words.
DOCLING_VENV_PYTHON="$HOME/.venvs/pdfextractor-docling/bin/python"

# `$1` is 1 when the run needs an oracle (the release gate) and 0 when it takes
# the venv if there is one without requiring it (a shard, or `--no-run`).
ensure_docling_oracle() {
    local strict=$1
    if [[ -n ${DOCLING_PY:-} ]]; then
        note "docling oracle: DOCLING_PY=$DOCLING_PY"
        return 0
    fi
    case "${DOCLING_SKIP_ORACLE:-}" in
        1 | true | yes)
            note "docling oracle: opted out (DOCLING_SKIP_ORACLE=$DOCLING_SKIP_ORACLE); the schema layer alone is checked"
            return 0
            ;;
    esac
    if [[ -x $DOCLING_VENV_PYTHON ]]; then
        export DOCLING_PY=$DOCLING_VENV_PYTHON
        note "docling oracle: DOCLING_PY=$DOCLING_PY (the machine's venv)"
        return 0
    fi
    ((strict)) || return 0
    cat >&2 <<'EOF'
test: no interpreter for docling-core, so docling_core_accepts_our_documents
test: would fail - and a run that cannot load it must not report the corpus
test: accepted. Point DOCLING_PY at a python with docling-core installed:
test:   DOCLING_PY=~/.venvs/pdfextractor-docling/bin/python ./scripts/test.sh --full
test: or opt out in so many words, for a run that checks the schema alone:
test:   DOCLING_SKIP_ORACLE=1 ./scripts/test.sh --full
EOF
    exit 2
}

MODE=changed
BASE=HEAD
SHARD=""
JOBS=1
RERUN_FAILED=0
SINCE_SET=0
LIST_ONLY=0
EXPLAIN=0
JSON=0
SELF_TEST=0
RUNNER_SELF_TEST=0
TIMINGS_FILE="$ROOT/target/test-runner-timings.tsv"
FAILED_SPECS_FILE="$ROOT/target/test-runner-failed.tsv"
FAILED_SPECS=()
FIXTURE_OWNERS="$ROOT/scripts/fixture-owners.tsv"
TIMING_COMMANDS=()
TIMING_MILLIS=()
WANTED_PACKAGES=()
WANTED_PATHS=()
CARGO_TEST_OPTIONS=()
TEST_FILTERS=()
CARGO_TEST_PASSTHROUGH=()
NO_RUN=0
NO_RUN_DOC_SKIPPED=0
BATCH=0
BATCH_SPECS=()
declare -A BATCH_TARGETS=()
DIRECT_TARGETS=()
EXPLICIT_TARGETS=()
PACKAGE_DIRS=()
SELECTED=()
COMMANDS=()
SORTED_COMMANDS=()
SELECT_ALL=0
NO_TEST_PATHS=()

while (($#)); do
    case "$1" in
        --changed)
            MODE=changed
            shift
            ;;
        --full|--all)
            MODE=full
            shift
            ;;
        --since)
            [[ $# -ge 2 ]] || die "--since needs a revision"
            BASE=$2
            SINCE_SET=1
            shift 2
            ;;
        --rerun-failed)
            RERUN_FAILED=1
            shift
            ;;
        --shard)
            [[ $# -ge 2 ]] || die "--shard needs I/N"
            SHARD=$2
            shift 2
            ;;
        --jobs|-j)
            [[ $# -ge 2 ]] || die "--jobs needs a target count"
            JOBS=$2
            shift 2
            ;;
        --path)
            [[ $# -ge 2 ]] || die "--path needs a file path"
            WANTED_PATHS+=("$2")
            shift 2
            ;;
        --package|-p)
            [[ $# -ge 2 ]] || die "--package needs a crate name"
            WANTED_PACKAGES+=("$2")
            shift 2
            ;;
        --features)
            [[ $# -ge 2 && -n $2 ]] || die "--features needs a feature list"
            CARGO_TEST_OPTIONS+=(--features "$2")
            shift 2
            ;;
        --no-run)
            CARGO_TEST_OPTIONS+=(--no-run)
            NO_RUN=1
            shift
            ;;
        --no-fail-fast)
            CARGO_TEST_OPTIONS+=(--no-fail-fast)
            shift
            ;;
        --filter)
            [[ $# -ge 2 && -n $2 ]] || die "--filter needs a test name"
            TEST_FILTERS+=("$2")
            shift 2
            ;;
        --batch)
            BATCH=1
            shift
            ;;
        --)
            shift
            CARGO_TEST_PASSTHROUGH+=("$@")
            break
            ;;
        --list)
            LIST_ONLY=1
            shift
            ;;
        --explain)
            EXPLAIN=1
            shift
            ;;
        --json)
            JSON=1
            shift
            ;;
        --self-test)
            SELF_TEST=1
            shift
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            die "unknown option: $1 (try --help)"
            ;;
    esac
done

if ((SELF_TEST)); then
    exec "$ROOT/scripts/test_runner_test.sh"
fi

if ((JSON && !LIST_ONLY && !EXPLAIN)); then
    die "--json needs --list or --explain"
fi

if ((BATCH && JSON)); then
    die "--batch groups commands; --json still describes individual targets"
fi
if ((BATCH)) && [[ $MODE == full && -z $SHARD ]]; then
    die "--batch groups selected targets; --full is already one cargo invocation"
fi

if [[ $MODE == full && ${#WANTED_PACKAGES[@]} -gt 0 ]]; then
    die "--package selects a fast run; --full names the whole suite"
fi
if [[ $MODE == full && ${#WANTED_PATHS[@]} -gt 0 ]]; then
    die "--path selects a fast run; --full names the whole suite"
fi
if ((RERUN_FAILED)); then
    [[ $MODE == changed ]] \
        || die "--rerun-failed is its own mode; do not combine it with --full"
    ((${#WANTED_PACKAGES[@]} == 0 && ${#WANTED_PATHS[@]} == 0)) \
        || die "--rerun-failed reruns the recorded targets; do not combine it with --package or --path"
    ((SINCE_SET == 0)) \
        || die "--rerun-failed reruns the recorded targets; do not combine it with --since"
    [[ -z $SHARD ]] \
        || die "--rerun-failed reruns the recorded targets; do not combine it with --shard"
fi
[[ $JOBS =~ ^[0-9]+$ ]] || die "--jobs must be a positive whole number"
JOBS=$((10#$JOBS))
((JOBS > 0)) || die "--jobs must be at least 1"

if [[ -n $SHARD ]]; then
    [[ $SHARD == */* ]] || die "--shard must be I/N, e.g. 1/4"
    shard_index=${SHARD%%/*}
    shard_count=${SHARD#*/}
    [[ $shard_index =~ ^[0-9]+$ && $shard_count =~ ^[0-9]+$ ]] \
        || die "--shard must be two positive integers, e.g. 1/4"
    shard_index=$((10#$shard_index))
    shard_count=$((10#$shard_count))
    ((shard_count > 0)) || die "--shard's count must be greater than zero"
    ((shard_index >= 1 && shard_index <= shard_count)) \
        || die "--shard $SHARD is outside 1..$shard_count"
fi

# A full run with no shard needs no metadata or target selection. Keep it the
# exact release command and do not make it depend on the JSON parser below.
if [[ $MODE == full && -z $SHARD ]]; then
    ((JOBS == 1)) \
        || die "--jobs parallelizes selected targets; --full without --shard is one workspace command"
    if ((JSON)); then
        emit_workspace_json
        exit 0
    fi
    if [[ $LIST_ONLY -eq 1 ]]; then
        workspace_command_line
        exit 0
    fi
    note "running the explicit full workspace suite"
    if ((NO_RUN)); then
        ensure_docling_oracle 0
    else
        ensure_docling_oracle 1
    fi
    full_args=(test --workspace --locked)
    full_args+=("${CARGO_TEST_OPTIONS[@]}" "${TEST_FILTERS[@]}")
    if ((${#CARGO_TEST_PASSTHROUGH[@]} > 0)); then
        full_args+=(-- "${CARGO_TEST_PASSTHROUGH[@]}")
    fi
    exec cargo "${full_args[@]}"
fi

# Cargo is the authority on workspace members, targets and path dependencies.
# Text parsing a manifest missed custom targets, dependencies under a target
# table and `package = "..."` renames; metadata carries all of them. `--no-deps`
# keeps this a local workspace read and does not fetch the registry.
command -v python3 >/dev/null 2>&1 \
    || die "cargo metadata parsing needs python3"
metadata=$(
    cargo metadata --no-deps --format-version 1 \
        | python3 -c '
import json
import os
import re
import subprocess
import sys

root = os.path.realpath(sys.argv[1])
data = json.load(sys.stdin)
members = set(data.get("workspace_members", []))

# Cargo metadata lists target-specific dependencies even when their predicate
# is false for this host. Read the host cfg set from rustc and skip
# inactive edges; if rustc is unavailable, keep the edge rather than silently
# dropping a dependency the eventual cargo test could still observe.
def host_cfg():
    try:
        output = subprocess.check_output(
            ["rustc", "--print", "cfg"],
            text=True,
            stderr=subprocess.DEVNULL,
        )
    except (OSError, subprocess.CalledProcessError):
        return None
    values = set()
    for line in output.splitlines():
        line = line.strip()
        if not line:
            continue
        if "=" in line:
            key, value = line.split("=", 1)
            values.add((key, value.strip().strip("\"")))
        else:
            values.add((line, None))
    return values


def host_triple():
    try:
        output = subprocess.check_output(
            ["rustc", "-vV"], text=True, stderr=subprocess.DEVNULL
        )
    except (OSError, subprocess.CalledProcessError):
        return None
    for line in output.splitlines():
        if line.startswith("host: "):
            return line[6:].strip()
    return None


CFG = host_cfg()
HOST = host_triple()


def cfg_value(tokens, index):
    token = tokens[index]
    index += 1
    if token in ("any", "all", "not"):
        if index >= len(tokens) or tokens[index] != "(":
            raise ValueError("missing cfg group")
        index += 1
        if token == "not":
            value, index = cfg_value(tokens, index)
            if index >= len(tokens) or tokens[index] != ")":
                raise ValueError("unterminated cfg not")
            return not value, index + 1
        values = []
        while index < len(tokens) and tokens[index] != ")":
            value, index = cfg_value(tokens, index)
            values.append(value)
            if index < len(tokens) and tokens[index] == ",":
                index += 1
        if index >= len(tokens):
            raise ValueError("unterminated cfg group")
        return (any(values) if token == "any" else all(values)), index + 1
    if index < len(tokens) and tokens[index] == "=":
        index += 1
        if index >= len(tokens):
            raise ValueError("missing cfg value")
        value = tokens[index].strip("\"")
        return (token, value) in CFG, index + 1
    return (token, None) in CFG, index


def target_is_active(target):
    if not target:
        return True
    if CFG is None:
        return True
    if not target.startswith("cfg("):
        return HOST is None or target == HOST
    expression = target[4:-1] if target.endswith(")") else target[4:]
    tokens = re.findall(
        r"[A-Za-z_][A-Za-z0-9_]*|\"(?:\\.|[^\"\\])*\"|[(),=]", expression
    )
    if not tokens:
        return True
    try:
        active, index = cfg_value(tokens, 0)
        return active if index == len(tokens) else True
    except (ValueError, IndexError):
        # An expression we cannot parse is safer as an included edge: the
        # full workspace is selected more often, never less often.
        return True

for package in data.get("packages", []):
    if package.get("id") not in members:
        continue
    manifest = package.get("manifest_path")
    if not manifest:
        continue
    package_dir = os.path.dirname(os.path.realpath(manifest))
    relative_dir = os.path.relpath(package_dir, root)
    print("member\t%s\t%s" % (relative_dir, package["name"]))

    for target in package.get("targets", []):
        source = target.get("src_path")
        if not source:
            continue
        relative_source = os.path.relpath(os.path.realpath(source), root)
        for kind in target.get("kind", []):
            if kind in ("lib", "proc-macro", "bin", "test"):
                print(
                    "target\t%s\t%s\t%s\t%s"
                    % (relative_dir, kind, target["name"], relative_source)
                )

    for dependency in package.get("dependencies", []):
        path = dependency.get("path")
        if not path or not target_is_active(dependency.get("target")):
            continue
        dependency_dir = os.path.realpath(path)
        relative_dependency = os.path.relpath(dependency_dir, root)
        if relative_dependency == ".." or relative_dependency.startswith(".." + os.sep):
            continue
        print("dep\t%s\t%s" % (relative_dir, relative_dependency))
' "$ROOT"
) || die "cargo metadata could not read the workspace"

declare -A PACKAGE_NAMES=()
declare -A PACKAGE_HAS_LIB=()
declare -A PACKAGE_HAS_BIN=()
declare -A PACKAGE_LIB_NAMES=()
declare -A PACKAGE_LIB_KINDS=()
declare -A PACKAGE_LIB_SOURCES=()
declare -A PACKAGE_BIN_NAMES=()
declare -A PACKAGE_BIN_SOURCES=()
declare -A PACKAGE_TESTS=()
declare -A TEST_TARGETS=()
declare -A TEST_TARGET_SOURCES=()
declare -A PACKAGE_REASONS=()
declare -A DIRECT_REASONS=()
declare -A EXPLICIT_REASONS=()
declare -A COMMAND_REASONS=()
declare -A PACKAGE_DEPS=()
SELF_TEST_REASON=""
SELECT_ALL_REASON=""
while IFS=$'\t' read -r kind relative_name first second third; do
    case "$kind" in
        member)
            [[ -n $relative_name && -n $first ]] || continue
            dir="$ROOT/$relative_name"
            [[ -d $dir ]] || die "missing workspace member $relative_name"
            PACKAGE_DIRS+=("$dir")
            PACKAGE_NAMES["$dir"]=$first
            ;;
        target)
            [[ -n $relative_name && -n $second && -n $third ]] || continue
            dir="$ROOT/$relative_name"
            case "$first" in
                lib|proc-macro)
                    PACKAGE_HAS_LIB["$dir"]=1
                    PACKAGE_LIB_NAMES["$dir"]=$second
                    PACKAGE_LIB_KINDS["$dir"]=$first
                    PACKAGE_LIB_SOURCES["$dir"]=$third
                    ;;
                bin)
                    PACKAGE_HAS_BIN["$dir"]=1
                    PACKAGE_BIN_NAMES["$dir"]+=$second$'\n'
                    PACKAGE_BIN_SOURCES["$dir|$second"]=$third
                    ;;
                test)
                    PACKAGE_TESTS["$dir"]+=$second$'\n'
                    TEST_TARGETS["$dir|$third"]=$second
                    TEST_TARGET_SOURCES["$dir|$second"]=$third
                    ;;
            esac
            ;;
        dep)
            [[ -n $relative_name && -n $first ]] || continue
            dir="$ROOT/$relative_name"
            PACKAGE_DEPS["$dir"]+="$ROOT/$first"$'\n'
            ;;
    esac
done <<< "$metadata"

((${#PACKAGE_DIRS[@]} > 0)) || die "cargo metadata reported no workspace members"

package_name() {
    printf '%s\n' "${PACKAGE_NAMES[$1]:-}"
}

test_target_for_path() {
    local key="$1|$2"
    printf '%s\n' "${TEST_TARGETS[$key]:-}"
}

package_dir() {
    local wanted=$1 dir
    for dir in "${PACKAGE_DIRS[@]}"; do
        if [[ $(package_name "$dir") == "$wanted" ]]; then
            printf '%s\n' "$dir"
            return 0
        fi
    done
    return 1
}

is_selected() {
    local dir=$1 existing
    for existing in "${SELECTED[@]}"; do
        [[ $existing == "$dir" ]] && return 0
    done
    return 1
}

add_selected() {
    local dir=$1 reason=${2:-"selected package $(package_name "$1")"}
    if is_selected "$dir"; then
        return 1
    fi
    SELECTED+=("$dir")
    PACKAGE_REASONS["$dir"]=$reason
    return 0
}

# The workspace path dependencies of one crate, as absolute directories.
# Cargo metadata has already expanded target tables and renamed dependencies,
# so every edge that can affect the crate is present here.
deps_of() {
    local dir=$1 dep
    while IFS= read -r dep; do
        [[ -n $dep ]] && printf '%s\n' "$dep"
    done <<< "${PACKAGE_DEPS[$dir]:-}"
    return 0
}

# The crate itself and every crate whose tests could observe it.
select_package_tree() {
    local root=$1 reason=${2:-"selected package $(package_name "$1")}"}
    local dir candidate
    local queue=("$root")
    local next=()
    add_selected "$root" "$reason" || true
    while ((${#queue[@]})); do
        next=()
        for dir in "${queue[@]}"; do
            for candidate in "${PACKAGE_DIRS[@]}"; do
                is_selected "$candidate" && continue
                if deps_of "$candidate" | grep -Fx "$dir" >/dev/null; then
                    add_selected "$candidate" "reverse dependency of $(package_name "$root")"
                    next+=("$candidate")
                fi
            done
        done
        queue=("${next[@]}")
    done
}

select_all_packages() {
    local reason=${1:-"full workspace selection"} dir
    for dir in "${PACKAGE_DIRS[@]}"; do
        add_selected "$dir" "$reason" || true
    done
}

direct_target() {
    local dir=$1 target=$2 reason=${3:-"direct target selection"} existing
    for existing in "${DIRECT_TARGETS[@]}"; do
        [[ $existing == "$dir|$target" ]] && return
    done
    DIRECT_TARGETS+=("$dir|$target")
    DIRECT_REASONS["$dir|$target"]=$reason
}

add_explicit_target() {
    local dir=$1 kind=$2 reason=${3:-"explicit target selection"} name spec existing
    name=$(package_name "$dir")
    spec="$dir|$name|$kind"
    for existing in "${EXPLICIT_TARGETS[@]}"; do
        [[ $existing == "$spec" ]] && return
    done
    EXPLICIT_TARGETS+=("$spec")
    EXPLICIT_REASONS["$spec"]=$reason
}

select_fixture_owner() {
    local owner=$1 reason=${2:-"fixture owner"} dir target
    # The owner is a source path, not a naming convention: a target declared
    # under examples/, checks/, or any other custom path is resolved through
    # cargo metadata by its exact source path.
    if [[ $owner =~ ^crates/([^/]+)/(.+\.rs)$ ]]; then
        dir="$ROOT/crates/${BASH_REMATCH[1]}"
        if [[ -f "$ROOT/$owner" && -f "$dir/Cargo.toml" ]]; then
            target=$(test_target_for_path "$dir" "$owner")
            if [[ -n $target ]]; then
                direct_target "$dir" "$target" "$reason (owner $owner)"
                return
            fi
        fi
    elif [[ $owner =~ ^crates/([^/]+)#lib$ ]]; then
        dir="$ROOT/crates/${BASH_REMATCH[1]}"
        if [[ -f "$dir/Cargo.toml" ]]; then
            add_explicit_target "$dir" lib "$reason (owner $owner)"
            return
        fi
    fi
    SELECT_ALL=1
    SELECT_ALL_REASON="$reason (unresolved owner $owner)"
}

# Whether a changed fixture path is covered by one ownership-table row.
#
# The row is a shell-style pattern, so `test-data/real/*.pdf` covers a file.
# A bare `dir/*` row is a directory wildcard: it covers every descendant and
# the directory itself, including nested or deleted paths that no longer exist.
# Treating the directory wildcard explicitly keeps it separate from an extension
# filter such as `dir/*.pdf`.
fixture_pattern_matches() {
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

# Fixture ownership lives in a table beside the runner. A match contributes
# only the targets that declare the fixture; a fixture with no row is unknown,
# so the broad fallback is still safe. The path need not exist: a deletion is
# still a change, and its old path is what the table describes.
select_fixture() {
    local path=${1%/} pattern owner
    local reason="fixture $path"
    local owners=()
    if [[ -f $FIXTURE_OWNERS ]]; then
        while IFS=$'\t' read -r pattern owner; do
            [[ -n $pattern && $pattern != \#* ]] || continue
            if fixture_pattern_matches "$path" "$pattern"; then
                owners+=("$owner")
            fi
        done <"$FIXTURE_OWNERS"
    fi
    if ((${#owners[@]} == 0)); then
        SELECT_ALL=1
        SELECT_ALL_REASON="unowned $reason"
        return
    fi
    for owner in "${owners[@]}"; do
        select_fixture_owner "$owner" "$reason"
    done
}

add_documentation_test() {
    local reason=${1:-"documentation path"} cli
    if cli=$(package_dir pdfextractor-cli); then
        direct_target "$cli" documentation_test "$reason"
    else
        SELECT_ALL=1
        SELECT_ALL_REASON="$reason (no pdfextractor-cli package)"
    fi
}

add_runner_self_test() {
    RUNNER_SELF_TEST=1
    SELF_TEST_REASON=${1:-"scripts changed"}
}

# The package directory a `crates/<name>/...` path belongs to, if it is one.
dir_from_path() {
    local path=$1 rest pkg dir
    [[ $path == crates/* ]] || return 1
    rest=${path#crates/}
    pkg=${rest%%/*}
    [[ $pkg == "$rest" ]] && return 1
    dir="$ROOT/crates/$pkg"
    [[ -f "$dir/Cargo.toml" ]] || return 1
    printf '%s\n' "$dir"
}

select_path() {
    local path=$1 dir cli_name
    case "$path" in
        Cargo.toml|Cargo.lock|rust-toolchain*|.cargo/*)
            SELECT_ALL=1
            SELECT_ALL_REASON="broad input $path"
            ;;
        scripts|scripts/*)
            add_runner_self_test "changed path $path"
            ;;
        crates/*)
            if dir=$(dir_from_path "$path"); then
                # A test target's source is selected directly, whether it is
                # auto-discovered or declared with a custom name and path. A
                # helper beside it selects the whole package tree.
                target=$(test_target_for_path "$dir" "$path")
                if [[ -n $target ]]; then
                    direct_target "$dir" "$target" "changed path $path"
                else
                    select_package_tree "$dir" "changed path $path"
                fi
            else
                SELECT_ALL=1
                SELECT_ALL_REASON="unknown crate path $path"
            fi
            ;;
        test-data/*)
            select_fixture "$path"
            ;;
        README.md|plan.md|progress.md|requirements.md|AGENTS.md|docs/*)
            # The documentation test holds the counts and reference markers in
            # these files to the real test list.
            add_documentation_test "documentation path $path"
            ;;
        .github/*|.freebuff/*|dist/*|target/*|fuzz/*)
            NO_TEST_PATHS+=("$path")
            ;;
        *)
            SELECT_ALL=1
            SELECT_ALL_REASON="unrecognized path $path"
            ;;
    esac
}

changed_paths=()
if ((RERUN_FAILED)); then
    # The failed-target state is loaded after the command helpers are defined.
    :
elif [[ $MODE == changed ]]; then
    for wanted in "${WANTED_PACKAGES[@]}"; do
        dir=$(package_dir "$wanted") || die "unknown package: $wanted"
        add_selected "$dir" "--package $wanted" || true
    done
    if [[ ${#WANTED_PATHS[@]} -gt 0 ]]; then
        for path in "${WANTED_PATHS[@]}"; do
            path=${path#"$ROOT"/}
            path=${path#./}
            changed_paths+=("$path")
        done
    elif [[ ${#WANTED_PACKAGES[@]} -eq 0 ]]; then
        git rev-parse --verify "$BASE^{commit}" >/dev/null 2>&1 \
            || die "--since $BASE is not a revision in this checkout"
        merge_base=$(git merge-base "$BASE" HEAD) \
            || die "--since $BASE has no merge base with HEAD"
        merge_base=${merge_base%%$'\n'*}
        # A rename changes both paths. Ask Git for the deletion and addition
        # separately so the old owner's tests are selected too, instead of only
        # the new path that --name-only reports for a detected rename.
        while IFS= read -r path; do
            [[ -n $path ]] && changed_paths+=("$path")
        done < <(git diff --no-renames --name-only "$merge_base" --)
        while IFS= read -r path; do
            [[ -n $path ]] && changed_paths+=("$path")
        done < <(git ls-files --others --exclude-standard)
    fi

    if ((${#changed_paths[@]} == 0 && ${#WANTED_PACKAGES[@]} == 0)); then
        note "no files changed against $BASE; nothing selected (use --full or --since REV)"
    fi
    for path in "${changed_paths[@]}"; do
        select_path "$path"
    done
    if ((SELECT_ALL)); then
        note "a broad input selected the full workspace; use --full explicitly for a release check"
        select_all_packages "${SELECT_ALL_REASON:-broad selection}"
    fi
else
    select_all_packages "--full${SHARD:+ --shard $SHARD}"
fi

if [[ $EXPLAIN -eq 1 && $JSON -eq 0 ]]; then
    if ((${#changed_paths[@]})); then
        note "changed against $BASE: ${changed_paths[*]}"
    fi
    for path in "${NO_TEST_PATHS[@]}"; do
        note "no Rust test selected for $path"
    done
    if ((RUNNER_SELF_TEST)); then
        note "selected: test-runner self-tests"
    fi
    for dir in "${SELECTED[@]}"; do
        note "selected package: $(package_name "$dir")"
    done
    for target in "${DIRECT_TARGETS[@]}"; do
        note "selected target: ${target%%|*}/tests/${target#*|}.rs"
    done
fi

add_command() {
    local command=$1 reason=${2:-"selected target"} existing
    for existing in "${COMMANDS[@]}"; do
        [[ $existing == "$command" ]] && return
    done
    COMMANDS+=("$command")
    COMMAND_REASONS["$command"]=$reason
}

build_commands() {
    local dir name test reason target
    for dir in "${SELECTED[@]}"; do
        name=$(package_name "$dir")
        reason=${PACKAGE_REASONS[$dir]:-"selected package $name"}
        if [[ ${PACKAGE_HAS_LIB[$dir]:-0} == 1 ]]; then
            add_command "$dir|$name|lib" "$reason"
            add_command "$dir|$name|doc" "$reason"
        fi
        if [[ ${PACKAGE_HAS_BIN[$dir]:-0} == 1 ]]; then
            add_command "$dir|$name|bins" "$reason"
        fi
        while IFS= read -r test; do
            [[ -n $test ]] || continue
            add_command "$dir|$name|test:$test" "$reason"
        done <<< "${PACKAGE_TESTS[$dir]:-}"
    done
    for target in "${DIRECT_TARGETS[@]}"; do
        dir=${target%%|*}
        test=${target#*|}
        name=$(package_name "$dir")
        add_command "$dir|$name|test:$test" \
            "${DIRECT_REASONS[$target]:-direct test target}"
    done
    for target in "${EXPLICIT_TARGETS[@]}"; do
        add_command "$target" "${EXPLICIT_REASONS[$target]:-explicit target}"
    done
    if ((RUNNER_SELF_TEST)); then
        add_command "self-test" "${SELF_TEST_REASON:-scripts changed}"
    fi
}

build_commands

if ((${#COMMANDS[@]})); then
    while IFS= read -r command; do
        [[ -n $command ]] && SORTED_COMMANDS+=("$command")
    done < <(printf '%s\n' "${COMMANDS[@]}" | LC_ALL=C sort)
    COMMANDS=("${SORTED_COMMANDS[@]}")
fi

# Cargo rejects `--doc --no-run`; there is no separate doc-test executable to
# build. Skip doc targets in that mode instead of emitting a command that fails.
if ((NO_RUN)); then
    no_run_commands=()
    for command in "${COMMANDS[@]}"; do
        if [[ $command == *'|doc' ]]; then
            NO_RUN_DOC_SKIPPED=1
            continue
        fi
        no_run_commands+=("$command")
    done
    COMMANDS=("${no_run_commands[@]}")
fi

command_line() {
    local spec=$1 dir name kind line package kindfield
    local -a batch_kinds
    if [[ $spec == self-test ]]; then
        printf './scripts/test_runner_test.sh\n'
        return
    fi
    if [[ $spec == batch\|* ]]; then
        IFS='|' read -r _ package kindfield <<< "$spec"
        printf 'cargo test -p %s' "$package"
        IFS=',' read -r -a batch_kinds <<< "$kindfield"
        for kind in "${batch_kinds[@]}"; do
            case "$kind" in
                lib) printf ' --lib' ;;
                bins) printf ' --bins' ;;
                doc) printf ' --doc' ;;
                test:*)
                    printf ' --test '
                    display_arg "${kind#test:}"
                    ;;
                *) die "internal error: unknown batch target kind $kind" ;;
            esac
        done
        printf ' --locked'
        cargo_extra_command_line
        printf '\n'
        return
    fi
    IFS='|' read -r dir name kind <<< "$spec"
    case "$kind" in
        lib) line="cargo test -p $name --lib --locked" ;;
        bins) line="cargo test -p $name --bins --locked" ;;
        doc) line="cargo test -p $name --doc --locked" ;;
        test:*) line="cargo test -p $name --test ${kind#test:} --locked" ;;
        *) die "internal error: unknown target kind $kind" ;;
    esac
    printf '%s' "$line"
    cargo_extra_command_line
    printf '\n'
}

# One JSON record per selected cargo invocation. A package's --bins command is
# expanded into its actual binary targets so every record has a target name and
# source path; --doc stays a pseudo-target because Cargo exposes no doc target
# in metadata.
emit_json_target() {
    local spec=$1 dir name kind reason source command target_name bins bin
    if [[ $spec == self-test ]]; then
        reason=${COMMAND_REASONS[$spec]:-${SELF_TEST_REASON:-scripts changed}}
        [[ -n $SHARD ]] && reason="$reason; shard $SHARD"
        printf 'target\t\t%s\t%s\t%s\t%s\t%s\n' \
            "self-test" "self-test" "scripts/test_runner_test.sh" "$reason" \
            "$(command_line "$spec")"
        return
    fi
    IFS='|' read -r dir name kind <<< "$spec"
    reason=${COMMAND_REASONS[$spec]:-selected target}
    if ((RERUN_FAILED)) && [[ -z ${COMMAND_REASONS[$spec]:-} ]]; then
        reason="rerun of previously failed target"
    fi
    [[ -n $SHARD ]] && reason="$reason; shard $SHARD"
    command=$(command_line "$spec")
    case "$kind" in
        lib)
            target_name=${PACKAGE_LIB_NAMES[$dir]:-$name}
            source=${PACKAGE_LIB_SOURCES[$dir]:-}
            printf 'target\t%s\t%s\t%s\t%s\t%s\t%s\n' \
                "$name" "${PACKAGE_LIB_KINDS[$dir]:-lib}" "$target_name" \
                "$source" "$reason" "$command"
            ;;
        doc)
            source=${PACKAGE_LIB_SOURCES[$dir]:-}
            printf 'target\t%s\tdoc\tdoc\t%s\t%s\t%s\n' \
                "$name" "$source" "$reason" "$command"
            ;;
        bins)
            bins=${PACKAGE_BIN_NAMES[$dir]:-}
            if [[ -z $bins ]]; then
                printf 'target\t%s\tbin\tbins\t\t%s\t%s\n' \
                    "$name" "$reason" "$command"
            else
                while IFS= read -r bin; do
                    [[ -n $bin ]] || continue
                    local bin_key="$dir|$bin"
                    printf 'target\t%s\tbin\t%s\t%s\t%s\t%s\n' \
                        "$name" "$bin" "${PACKAGE_BIN_SOURCES[$bin_key]:-}" \
                        "$reason" "$command"
                done <<< "$bins"
            fi
            ;;
        test:*)
            target_name=${kind#test:}
            local test_key="$dir|$target_name"
            source=${TEST_TARGET_SOURCES[$test_key]:-}
            printf 'target\t%s\ttest\t%s\t%s\t%s\t%s\n' \
                "$name" "$target_name" "$source" "$reason" "$command"
            ;;
        *)
            die "internal error: unknown target kind $kind"
            ;;
    esac
}

emit_json() {
    local mode=$MODE path spec
    local -a targets=("$@")
    ((RERUN_FAILED)) && mode=rerun-failed
    command -v python3 >/dev/null 2>&1 \
        || die "JSON output needs python3"
    {
        for path in "${changed_paths[@]}"; do
            printf 'path\t%s\n' "$path"
        done
        for spec in "${targets[@]}"; do
            emit_json_target "$spec"
        done
    } | python3 -c '
import json
import sys

mode, base, shard = sys.argv[1:4]
changed_paths = []
targets = []
for line in sys.stdin:
    fields = line.rstrip("\n").split("\t")
    if fields[0] == "path" and len(fields) >= 2:
        changed_paths.append(fields[1])
    elif fields[0] == "target" and len(fields) >= 7:
        package, kind, name, source, reason, command = fields[1:7]
        targets.append({
            "package": package or None,
            "kind": kind,
            "name": name,
            "source_path": source or None,
            "reason": reason,
            "command": command,
        })

document = {
    "mode": mode,
    "base": base,
    "changed_paths": changed_paths,
    "targets": targets,
}
if shard:
    document["shard"] = shard
print(json.dumps(document, indent=2))
' "$mode" "$BASE" "$SHARD"
}

run_command() {
    local spec=$1 dir name kind kindfield
    local -a args batch_kinds
    if [[ $spec == self-test ]]; then
        "$ROOT/scripts/test_runner_test.sh"
        return
    fi
    if [[ $spec == batch\|* ]]; then
        IFS='|' read -r _ name kindfield <<< "$spec"
        args=(test -p "$name")
        IFS=',' read -r -a batch_kinds <<< "$kindfield"
        for kind in "${batch_kinds[@]}"; do
            case "$kind" in
                lib) args+=(--lib) ;;
                bins) args+=(--bins) ;;
                doc) args+=(--doc) ;;
                test:*) args+=(--test "${kind#test:}") ;;
                *) die "internal error: unknown batch target kind $kind" ;;
            esac
        done
        args+=(--locked)
        args+=("${CARGO_TEST_OPTIONS[@]}" "${TEST_FILTERS[@]}")
        if ((${#CARGO_TEST_PASSTHROUGH[@]} > 0)); then
            args+=(-- "${CARGO_TEST_PASSTHROUGH[@]}")
        fi
        cargo "${args[@]}"
        return
    fi
    IFS='|' read -r dir name kind <<< "$spec"
    case "$kind" in
        lib) args=(test -p "$name" --lib --locked) ;;
        bins) args=(test -p "$name" --bins --locked) ;;
        doc) args=(test -p "$name" --doc --locked) ;;
        test:*) args=(test -p "$name" --test "${kind#test:}" --locked) ;;
        *) die "internal error: unknown target kind $kind" ;;
    esac
    args+=("${CARGO_TEST_OPTIONS[@]}" "${TEST_FILTERS[@]}")
    if ((${#CARGO_TEST_PASSTHROUGH[@]} > 0)); then
        args+=(-- "${CARGO_TEST_PASSTHROUGH[@]}")
    fi
    cargo "${args[@]}"
}

# Batch mode is applied after sharding and rerun-failed selection. A non-doc
# group contains every selected non-doc target for one package; docs are a
# separate group because Cargo rejects --doc with other selectors. The encoded
# selector list is passed to one cargo invocation. Cargo accepts repeated
# --test/--bin style selectors, so a group does not widen to unselected targets.
build_batch_specs() {
    local spec dir name kind group item key group_kind
    local -a order=()
    declare -A key_kinds=()
    declare -A key_members=()
    BATCH_SPECS=()
    BATCH_TARGETS=()
    for spec in "${RUN_SPECS[@]}"; do
        if [[ $spec == self-test ]]; then
            order+=("spec|$spec")
            continue
        fi
        IFS='|' read -r dir name kind <<< "$spec"
        # cargo rejects --doc when it is combined with any other target selector,
        # so a package with both docs and other targets needs two groups.
        if [[ $kind == doc ]]; then
            group_kind=doc
        else
            group_kind=targets
        fi
        key="$dir|$name|$group_kind"
        if [[ -z ${key_members[$key]:-} ]]; then
            order+=("key|$key")
            key_members[$key]=$spec
            key_kinds[$key]=$kind
        else
            key_members[$key]+=$'\n'"$spec"
            key_kinds[$key]+=",$kind"
        fi
    done
    for item in "${order[@]}"; do
        case "$item" in
            spec\|*)
                BATCH_SPECS+=("${item#spec|}")
                ;;
            key\|*)
                key=${item#key|}
                IFS='|' read -r dir name _ <<< "$key"
                group="batch|$name|${key_kinds[$key]}"
                BATCH_SPECS+=("$group")
                BATCH_TARGETS["$group"]=${key_members[$key]}
                ;;
        esac
    done
}

failed_state_has() {
    local wanted=$1 existing
    for existing in "${FAILED_SPECS[@]}"; do
        [[ $existing == "$wanted" ]] && return 0
    done
    return 1
}

add_failed_specs_for_spec() {
    local spec=$1 member
    while IFS= read -r member; do
        [[ -n $member ]] || continue
        failed_state_has "$member" || FAILED_SPECS+=("$member")
    done < <(target_specs_for_spec "$spec")
}

remove_passed_specs_for_spec() {
    local spec=$1 existing member remove
    local -a remaining=()
    while IFS= read -r existing; do
        [[ -n $existing ]] || continue
        remove=0
        while IFS= read -r member; do
            [[ $existing == "$member" ]] && remove=1 && break
        done < <(target_specs_for_spec "$spec")
        ((remove == 0)) && remaining+=("$existing")
    done <<< "$(printf '%s\n' "${FAILED_SPECS[@]}")"
    FAILED_SPECS=("${remaining[@]}")
}

# Update the persistent set only for targets that actually ran. A sequential
# fail-fast leaves later selected targets untouched, and targets outside the
# current selection must keep their old failure state.
update_failed_state_for_spec() {
    local spec=$1 status=$2
    ((NO_RUN == 0)) || return 0
    if ((status == 0)); then
        remove_passed_specs_for_spec "$spec"
    else
        add_failed_specs_for_spec "$spec"
    fi
}

persist_failed_specs() {
    if ((${#FAILED_SPECS[@]} > 0)); then
        write_failed_specs "${FAILED_SPECS[@]}"
    else
        clear_failed_specs
    fi
}

# A batch command is one cargo process for several target specs. Failure state
# must still name the individual targets so a later --rerun-failed can retry
# only the affected package group without losing the target list.
target_specs_for_spec() {
    local spec=$1
    if [[ $spec == batch\|* ]]; then
        [[ -n ${BATCH_TARGETS[$spec]:-} ]] \
            && printf '%s\n' "${BATCH_TARGETS[$spec]}"
    else
        printf '%s\n' "$spec"
    fi
}

# A group has one elapsed time but several target records. Spread that time
# evenly across its members so the existing per-target timing table and shard
# estimates remain populated without claiming precision the group cannot know.
record_timings_for_spec() {
    local spec=$1 took=$2 member share
    local -a members=()
    if [[ $spec == batch\|* ]]; then
        while IFS= read -r member; do
            [[ -n $member ]] && members+=("$member")
        done <<< "${BATCH_TARGETS[$spec]:-}"
        ((${#members[@]} > 0)) || return 0
        share=$((took / ${#members[@]}))
        ((share > 0)) || share=1
        for member in "${members[@]}"; do
            record_timing "$(command_line "$member")" "$share" || true
        done
    else
        record_timing "$(command_line "$spec")" "$took" || true
    fi
}

load_timings() {
    local value command millis index current
    [[ -f $TIMINGS_FILE ]] || return 0
    while IFS=$'\t' read -r value command; do
        [[ $value =~ ^[0-9]+(\.[0-9]+)?$ && -n $command ]] || continue
        millis=$(timing_to_millis "$value")
        index=-1
        for ((current = 0; current < ${#TIMING_COMMANDS[@]}; current++)); do
            if [[ ${TIMING_COMMANDS[current]} == "$command" ]]; then
                index=$current
                break
            fi
        done
        if ((index >= 0)); then
            TIMING_MILLIS[index]=$millis
        else
            TIMING_COMMANDS+=("$command")
            TIMING_MILLIS+=("$millis")
        fi
    done <"$TIMINGS_FILE"
}

timing_for() {
    local command=$1 index
    for ((index = 0; index < ${#TIMING_COMMANDS[@]}; index++)); do
        if [[ ${TIMING_COMMANDS[index]} == "$command" ]]; then
            printf '%s\n' "${TIMING_MILLIS[index]}"
            return
        fi
    done
    printf '0\n'
}

# Convert a timing file's seconds, with or without a fractional part, to
# integer milliseconds. Existing whole-second recordings stay valid, and a
# sub-second recording is not rounded down to zero.
timing_to_millis() {
    local value=$1 whole fraction millis
    whole=${value%%.*}
    [[ -n $whole ]] || whole=0
    if [[ $value == *.* ]]; then
        fraction=${value#*.}
    else
        fraction=0
    fi
    fraction=${fraction}000
    millis=$((10#$whole * 1000 + 10#${fraction:0:3}))
    # An old whole-second recording can be zero. Treat it as a real sub-second
    # timing rather than dropping the target back to the count-based split.
    ((millis > 0)) || millis=1
    printf '%s\n' "$millis"
}

# Estimate the cost of a target that has never run from the recorded costs of
# the targets that have. The median keeps one unusually slow target from making
# every new target look equally slow. Callers only use this when at least one
# real recording exists.
median_timing() {
    local count=$# midpoint value
    local -a sorted=()
    ((count > 0)) || {
        printf '0\n'
        return
    }
    while IFS= read -r value; do
        sorted+=("$value")
    done < <(printf '%s\n' "$@" | LC_ALL=C sort -n)
    midpoint=$((count / 2))
    if ((count % 2 == 1)); then
        printf '%s\n' "${sorted[midpoint]}"
    else
        printf '%s\n' "$(((${sorted[midpoint - 1]} + ${sorted[midpoint]}) / 2))"
    fi
}

# A wall clock in milliseconds. Bash 5's EPOCHREALTIME is exact and needs no
# process; GNU date's nanoseconds are the next cheapest. BSD/macOS date does
# not implement %N, so Python and Perl are fallbacks there. Whole seconds are
# the final portable floor when none of those is available.
now_millis() {
    local stamp whole fraction value
    if [[ -n ${EPOCHREALTIME:-} ]]; then
        stamp=$EPOCHREALTIME
        whole=${stamp%%.*}
        fraction=${stamp#*.}
        fraction=${fraction}000
        printf '%s\n' "$((10#$whole * 1000 + 10#${fraction:0:3}))"
        return
    fi
    if stamp=$(date +%s%N 2>/dev/null) && [[ $stamp =~ ^[0-9]{16,}$ ]]; then
        printf '%s\n' "$((10#${stamp:0:${#stamp}-6}))"
        return
    fi
    if command -v python3 >/dev/null 2>&1; then
        if value=$(python3 -c 'import time; print(int(time.time() * 1000))' 2>/dev/null) \
            && [[ $value =~ ^[0-9]+$ ]]; then
            printf '%s\n' "$value"
            return
        fi
    fi
    if command -v perl >/dev/null 2>&1; then
        if value=$(perl -MTime::HiRes=time -e 'printf "%d\n", time() * 1000' 2>/dev/null) \
            && [[ $value =~ ^[0-9]+$ ]]; then
            printf '%s\n' "$value"
            return
        fi
    fi
    printf '%s000\n' "$(date +%s)"
}

record_timing() {
    local command=$1 millis=$2
    [[ $millis =~ ^[0-9]+$ ]] || return 0
    mkdir -p "${TIMINGS_FILE%/*}" 2>/dev/null || return 0
    printf '%d.%03d\t%s\n' "$((millis / 1000))" "$((millis % 1000))" "$command" >>"$TIMINGS_FILE" 2>/dev/null || return 0
}

clear_failed_specs() {
    rm -f "$FAILED_SPECS_FILE"
}

# Record the internal target specs, not command lines: the next --rerun-failed
# can then reconstruct the same commands without parsing cargo's output.
write_failed_specs() {
    local spec existing tmp duplicate
    if (($# == 0)); then
        clear_failed_specs
        return 0
    fi
    mkdir -p "${FAILED_SPECS_FILE%/*}" 2>/dev/null || return 0
    tmp="$FAILED_SPECS_FILE.tmp.$$"
    : >"$tmp" 2>/dev/null || return 0
    local -a written=()
    for spec in "$@"; do
        duplicate=0
        for existing in "${written[@]}"; do
            [[ $existing == "$spec" ]] && duplicate=1 && break
        done
        if ((duplicate == 0)); then
            printf '%s\n' "$spec" >>"$tmp"
            written+=("$spec")
        fi
    done
    mv -f "$tmp" "$FAILED_SPECS_FILE" 2>/dev/null || rm -f "$tmp"
}

load_failed_specs() {
    local spec dir name kind existing duplicate
    FAILED_SPECS=()
    [[ -f $FAILED_SPECS_FILE ]] || return 0
    while IFS= read -r spec || [[ -n $spec ]]; do
        [[ -n $spec ]] || continue
        IFS='|' read -r dir name kind <<< "$spec"
        [[ -n $dir && -n $name && -n $kind ]] || continue
        case "$kind" in
            lib|bins|doc|test:*) ;;
            *) continue ;;
        esac
        duplicate=0
        for existing in "${FAILED_SPECS[@]}"; do
            [[ $existing == "$spec" ]] && duplicate=1 && break
        done
        ((duplicate == 0)) && FAILED_SPECS+=("$spec")
    done <"$FAILED_SPECS_FILE"
}

execute() {
    local spec=$1
    if [[ $LIST_ONLY -eq 1 ]]; then
        command_line "$spec"
    else
        printf '\n$ %s\n' "$(command_line "$spec")" >&2
        local started ended took status
        started=$(now_millis)
        if run_command "$spec"; then
            status=0
        else
            status=$?
        fi
        ended=$(now_millis)
        took=$((ended - started))
        ((took > 0)) || took=1
        # --no-run measures compilation, not test execution; do not feed that
        # into the runtime table that balances later test shards.
        if ((NO_RUN == 0)); then
            record_timings_for_spec "$spec" "$took"
        fi
        return "$status"
    fi
}

# Run one target with its stdout and stderr in one buffer. The buffer is
# printed by the parent after all parallel targets finish, so output from two
# targets is never interleaved on the terminal.
run_captured() {
    local spec=$1 log=$2
    {
        printf '\n$ %s\n' "$(command_line "$spec")"
        run_command "$spec"
    } >"$log" 2>&1
}

PARALLEL_TMP=""
cleanup_parallel() {
    if [[ -n $PARALLEL_TMP && -d $PARALLEL_TMP ]]; then
        rm -rf "$PARALLEL_TMP"
    fi
}
trap cleanup_parallel EXIT

# Each target is already an independent cargo invocation. Launch a bounded
# number at once, wait for all of them, then print their captured logs in the
# order the targets were selected. A failure does not cancel a sibling: a
# parallel run reports every target's output and returns the first failure's
# status, matching the sequential runner's failure signal without pretending
# the later targets never started.
run_parallel() {
    local -a specs=("$@") logs=() status_files=() timings=() statuses=()
    local index offset end log status_file wait_status status took member
    local failed=0 failed_status=0 failed_spec="" failed_count=0

    PARALLEL_TMP=$(mktemp -d "${TMPDIR:-/tmp}/pdfe-test-runner.XXXXXX") \
        || die "could not create a parallel log directory"

    for ((offset = 0; offset < ${#specs[@]}; offset += JOBS)); do
        local -a pids=() batch_indices=()
        end=$((offset + JOBS))
        ((end > ${#specs[@]})) && end=${#specs[@]}

        for ((index = offset; index < end; index++)); do
            log="$PARALLEL_TMP/$index.log"
            status_file="$PARALLEL_TMP/$index.status"
            logs[index]=$log
            status_files[index]=$status_file
            (
                set +e
                local started ended elapsed
                started=$(now_millis)
                run_captured "${specs[index]}" "$log"
                status=$?
                ended=$(now_millis)
                elapsed=$((ended - started))
                ((elapsed > 0)) || elapsed=1
                printf '%s\t%s\n' "$status" "$elapsed" >"$status_file"
                exit 0
            ) &
            pids+=("$!")
            batch_indices+=("$index")
        done

        local batch_position
        for batch_position in "${!pids[@]}"; do
            index=${batch_indices[batch_position]}
            set +e
            wait "${pids[batch_position]}"
            wait_status=$?
            set -e
            status=$wait_status
            took=1
            if [[ -s ${status_files[index]} ]]; then
                IFS=$'\t' read -r status took <"${status_files[index]}"
            else
                # The worker did not get to write its status (for example, it
                # was killed). Treat that as a failure rather than a silent pass.
                status=1
            fi
            statuses[index]=$status
            timings[index]=$took
        done
    done

    for index in "${!specs[@]}"; do
        status=${statuses[index]:-1}
        [[ -f ${logs[index]} ]] && cat "${logs[index]}"
        if ((NO_RUN == 0)); then
            record_timings_for_spec "${specs[index]}" "${timings[index]:-1}"
        fi
        update_failed_state_for_spec "${specs[index]}" "$status"
        if ((status != 0)); then
            failed=1
            while IFS= read -r member; do
                [[ -n $member ]] || continue
                failed_count=$((failed_count + 1))
            done < <(target_specs_for_spec "${specs[index]}")
            if ((failed_status == 0)); then
                failed_status=$status
                failed_spec=${specs[index]}
            fi
        fi
    done

    persist_failed_specs

    rm -rf "$PARALLEL_TMP"
    PARALLEL_TMP=""

    if ((failed)); then
        note "parallel: $failed_count target(s) failed; first was $(command_line "$failed_spec")"
        return "$failed_status"
    fi
}

RUN_SPECS=()
if ((RERUN_FAILED)); then
    load_failed_specs
    if ((${#FAILED_SPECS[@]} == 0)); then
        if ((JSON)); then
            emit_json
            exit 0
        fi
        note "no failed targets recorded; run a selected target first"
        exit 0
    fi
    RUN_SPECS=("${FAILED_SPECS[@]}")
    [[ $EXPLAIN -eq 1 ]] && note "rerunning recorded failed target(s)"
elif [[ -n $SHARD ]]; then
    total=${#COMMANDS[@]}
    if ((total == 0)); then
        if ((JSON)); then
            emit_json
            exit 0
        fi
        note "shard $SHARD has no selected test targets"
        exit 0
    fi
    load_timings
    known_timings=()
    for spec in "${COMMANDS[@]}"; do
        millis=$(timing_for "$(command_line "$spec")")
        ((millis > 0)) && known_timings+=("$millis")
    done
    if ((${#known_timings[@]} > 0)); then
        fallback_timing=$(median_timing "${known_timings[@]}")
        shard_totals=()
        for ((index = 0; index < shard_count; index++)); do
            shard_totals[index]=0
        done
        while IFS=$'\t' read -r millis spec; do
            target=0
            for ((candidate = 1; candidate < shard_count; candidate++)); do
                if ((shard_totals[candidate] < shard_totals[target])); then
                    target=$candidate
                fi
            done
            if ((target == shard_index - 1)); then
                RUN_SPECS+=("$spec")
            fi
            shard_totals[target]=$((shard_totals[target] + millis))
        done < <(
            for spec in "${COMMANDS[@]}"; do
                millis=$(timing_for "$(command_line "$spec")")
                ((millis > 0)) || millis=$fallback_timing
                printf '%s\t%s\n' "$millis" "$spec"
            done | LC_ALL=C sort -t $'\t' -k1,1nr -k2,2
        )
    else
        # No selected target has ever been recorded: count-based round-robin is
        # the only useful split when every target has the same unknown weight.
        for ((index = 0; index < total; index++)); do
            if ((index % shard_count == shard_index - 1)); then
                RUN_SPECS+=("${COMMANDS[$index]}")
            fi
        done
    fi
else
    RUN_SPECS=("${COMMANDS[@]}")
fi

if ((BATCH)); then
    build_batch_specs
    RUN_SPECS=("${BATCH_SPECS[@]}")
fi

if ((JSON)); then
    emit_json "${RUN_SPECS[@]}"
    exit 0
fi

if ((${#RUN_SPECS[@]} == 0)); then
    if [[ -n $SHARD ]]; then
        note "shard $SHARD has no selected test targets"
    else
        note "nothing selected; use --full for the release suite"
    fi
    exit 0
fi

if [[ $LIST_ONLY -eq 1 ]]; then
    for spec in "${RUN_SPECS[@]}"; do
        command_line "$spec"
    done
    exit 0
fi

# A normal run updates the existing unresolved set as targets actually run;
# rerun-failed already loaded that set for its selection above.
if ((RERUN_FAILED == 0)); then
    load_failed_specs
fi

if ((NO_RUN && NO_RUN_DOC_SKIPPED)); then
    note "skipping doc target(s): Cargo cannot combine --no-run with --doc"
fi
if ((BATCH)); then
    note "batch: ${#RUN_SPECS[@]} cargo invocation(s) for selected target(s)"
fi

# A shard of `--full` can hold the docling target as well: take the machine's
# venv when the caller named none, but do not stop a shard that does not.
if [[ $MODE == full ]]; then
    ensure_docling_oracle 0
fi

if ((JOBS > 1)); then
    note "running ${#RUN_SPECS[@]} selected target(s) with up to $JOBS at once"
    run_parallel "${RUN_SPECS[@]}"
    exit $?
fi

note "running ${#RUN_SPECS[@]} selected target(s)"
sequential_failed_status=0
for command in "${RUN_SPECS[@]}"; do
    set +e
    execute "$command"
    status=$?
    set -e
    update_failed_state_for_spec "$command" "$status"
    if ((status != 0)); then
        ((sequential_failed_status == 0)) && sequential_failed_status=$status
        if ((RERUN_FAILED == 0)); then
            # Keep the sequential fail-fast behavior for ordinary runs: the
            # first failure is the run's failure and later targets did not run.
            # Their old unresolved entries stay untouched, while this failure
            # and every earlier pass have already updated the set.
            persist_failed_specs
            exit "$status"
        fi
    fi
done
persist_failed_specs
if ((sequential_failed_status != 0)); then
    exit "$sequential_failed_status"
fi
