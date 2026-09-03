//! HPC job-script rules (JOB001–JOB005).
//!
//! These are the only rules that read two files. Everything else in xray
//! answers "is this line of Python a mistake?"; these answer "is this Python a
//! mistake *given what the job asked the scheduler for?*" — which is where HPC
//! jobs actually go wrong, and which no Python linter can see because the
//! resource request lives in a shell script.
//!
//! ## Shape
//!
//! Unlike the other domains, the query patterns do not map one-to-one onto
//! rules. `collect_facts` gathers what the Python does — the cluster it
//! builds, the datasets it opens, the pools it spawns — and each rule then
//! compares those facts against the parsed [`JobScript`]. That is the same
//! file-wide-state shape DK003 uses, just with a second input.
//!
//! ## The silence invariant
//!
//! A directive xray could not parse is `None`, and `None` never produces a
//! diagnostic. A job script that says `--cpus-per-task=$NCPUS` yields no core
//! count, so JOB001 stays quiet rather than guessing — being wrong about
//! somebody's allocation is far more expensive than saying nothing.

use std::sync::LazyLock;
use streaming_iterator::StreamingIterator;
use tree_sitter::{Node, Query, QueryCursor};

use crate::{
    config::Config,
    diagnostic::{Diagnostic, RuleMeta, Severity},
    job::{JobScript, format_bytes},
    parser::{
        ParsedFile, has_keyword_arg, keyword_arg_present_or_unknown, keyword_arg_value, node_text,
        position,
    },
};

pub struct JobRules;

const QUERY_SRC: &str = include_str!("../../queries/job.scm");

/// Compiled once per process and shared across all rayon workers.
/// A compilation failure is a bug in xray itself, so we fail loudly.
static QUERY: LazyLock<Query> = LazyLock::new(|| {
    Query::new(&tree_sitter_python::LANGUAGE.into(), QUERY_SRC)
        .unwrap_or_else(|e| panic!("xray: BUG — failed to compile job query: {e}"))
});

/// What the Python file does, as far as the JOB rules need to know.
#[derive(Debug, Default)]
struct PythonFacts {
    /// A dask cluster construction: its `n_workers=` literal if there was one,
    /// whether `threads_per_worker=` was set, and where it is.
    cluster: Option<ClusterFact>,
    /// Every `open_dataset` / `open_mfdataset` left without `chunks=`.
    unchunked_opens: Vec<(usize, usize, String)>,
    /// Every unbounded pool: `n_jobs=-1`, or a `Pool()` with no worker count.
    unbounded_pools: Vec<(usize, usize, String)>,
}

#[derive(Debug)]
struct ClusterFact {
    line: usize,
    column: usize,
    name: String,
    /// `None` when `n_workers=` was absent or not an integer literal.
    n_workers: Option<usize>,
    threads_per_worker: Option<usize>,
    /// `threads_per_worker=` present in any form, literal or not.
    threads_set: bool,
}

impl JobRules {
    pub fn meta() -> Vec<RuleMeta> {
        vec![
            RuleMeta {
                id: "JOB001",
                name: "allocation-cluster-mismatch",
                severity: Severity::Warning,
                description: "Allocated cores do not match the dask cluster the script builds — most of the allocation idles for the full wall time",
            },
            RuleMeta {
                id: "JOB002",
                name: "unpinned-thread-count",
                severity: Severity::Warning,
                description: "Multi-core request with no thread-count control — BLAS threads multiply by workers and oversubscribe the node",
            },
            RuleMeta {
                id: "JOB003",
                name: "memory-request-unchunked-read",
                severity: Severity::Error,
                description: "A memory request paired with an unchunked open_dataset/open_mfdataset — the OOM is predictable before the job is queued",
            },
            RuleMeta {
                id: "JOB004",
                name: "unused-gpu-allocation",
                severity: Severity::Warning,
                description: "A GPU was requested by a script that imports no GPU library — the allocation is billed and idle",
            },
            RuleMeta {
                id: "JOB005",
                name: "unbounded-worker-pool",
                severity: Severity::Warning,
                description: "n_jobs=-1 or an unbounded pool under a partial-node allocation — takes cores belonging to other jobs",
            },
        ]
    }

    /// Cross-check one Python file against the submission script that launches
    /// it. Called from `rules::run_all_with_job` only when `--job` (or
    /// `[job].script`) supplied one.
    ///
    /// JOB004 is deliberately *not* here — see [`JobRules::check_run`].
    pub fn check(
        file: &ParsedFile,
        path: &str,
        config: &Config,
        job: &JobScript,
    ) -> Vec<Diagnostic> {
        // A shell script with no `#SBATCH` / `#PBS` line at all carries no
        // resource request to check against.
        if !job.has_directives {
            return Vec::new();
        }

        let facts = collect_facts(file);
        let mut diags = Vec::new();

        job001(&facts, job, path, config, &mut diags);
        job002(&facts, job, path, config, &mut diags);
        job003(&facts, job, path, config, &mut diags);
        job005(&facts, job, path, config, &mut diags);

        diags
    }
}

/// JOB001 — the allocation and the cluster disagree.
fn job001(
    facts: &PythonFacts,
    job: &JobScript,
    path: &str,
    config: &Config,
    out: &mut Vec<Diagnostic>,
) {
    if config.is_disabled("JOB001") {
        return;
    }
    let (Some(cpus), Some(cluster)) = (job.cpus.as_ref(), facts.cluster.as_ref()) else {
        return;
    };
    let Some(n_workers) = cluster.n_workers else {
        return;
    };
    // The cluster's real core appetite is workers × threads-per-worker; dask
    // defaults threads_per_worker to one core each when unset.
    let threads = cluster.threads_per_worker.unwrap_or(1);
    let used = n_workers.saturating_mul(threads);
    if used == cpus.value {
        return;
    }

    let detail = if used < cpus.value {
        format!(
            "leaves {} of {} allocated core{} idle for the job's full wall time",
            cpus.value - used,
            cpus.value,
            if cpus.value == 1 { "" } else { "s" }
        )
    } else {
        format!(
            "oversubscribes the allocation — {used} slots on {} allocated core{}",
            cpus.value,
            if cpus.value == 1 { "" } else { "s" }
        )
    };

    out.push(
        Diagnostic::new(
            "JOB001",
            Severity::Warning,
            path,
            cluster.line,
            cluster.column,
            format!(
                "`{}` builds {n_workers} worker{} × {threads} thread{} while {} asks for {} core{} — {detail}",
                cluster.name,
                if n_workers == 1 { "" } else { "s" },
                if threads == 1 { "" } else { "s" },
                job.path,
                cpus.value,
                if cpus.value == 1 { "" } else { "s" },
            ),
        )
        .with_suggestion(format!(
            "Size the cluster from the allocation rather than a literal: `LocalCluster(n_workers=int(os.environ[\"SLURM_CPUS_PER_TASK\"]), threads_per_worker=1)`, or change the request to `--cpus-per-task={used}`",
        ))
        .with_url("https://github.com/greensh16/xray-cs/wiki/Job-Rules#job001"),
    );
}

/// JOB002 — a multi-core request with nothing capping BLAS threads.
fn job002(
    facts: &PythonFacts,
    job: &JobScript,
    path: &str,
    config: &Config,
    out: &mut Vec<Diagnostic>,
) {
    if config.is_disabled("JOB002") || !job.is_multicore() || job.thread_env.is_some() {
        return;
    }
    let Some(cpus) = job.cpus.as_ref() else {
        return;
    };
    // Either lever is enough: the job script exporting a thread cap, or the
    // Python pinning threads_per_worker.
    let Some(cluster) = facts.cluster.as_ref() else {
        return;
    };
    if cluster.threads_set {
        return;
    }

    let workers = cluster.n_workers.unwrap_or(cpus.value);
    out.push(
        Diagnostic::new(
            "JOB002",
            Severity::Warning,
            path,
            cluster.line,
            cluster.column,
            format!(
                "{} asks for {} cores and nothing caps the BLAS thread pool — neither `OMP_NUM_THREADS` in the job script nor `threads_per_worker=` here, so each of the {workers} workers may spawn {} BLAS threads",
                job.path, cpus.value, cpus.value
            ),
        )
        .with_suggestion(
            "Export `OMP_NUM_THREADS=1` (and `MKL_NUM_THREADS=1`) in the job script and let dask own the parallelism, or pass `threads_per_worker=` explicitly",
        )
        .with_url("https://github.com/greensh16/xray-cs/wiki/Job-Rules#job002"),
    );
}

/// JOB003 — a memory request paired with an unchunked read.
fn job003(
    facts: &PythonFacts,
    job: &JobScript,
    path: &str,
    config: &Config,
    out: &mut Vec<Diagnostic>,
) {
    if config.is_disabled("JOB003") {
        return;
    }
    let Some(mem) = job.memory_bytes.as_ref() else {
        return;
    };
    for (line, column, fn_name) in &facts.unchunked_opens {
        out.push(
            Diagnostic::new(
                "JOB003",
                Severity::Error,
                path,
                *line,
                *column,
                format!(
                    "`{fn_name}()` without `chunks=` loads eagerly, and {} caps this job at {} — the read either fits or the job is killed, with nothing in between",
                    job.path,
                    format_bytes(mem.value)
                ),
            )
            .with_suggestion(
                "Pass `chunks=` (or `chunks=\"auto\"`) so the read stays lazy and dask streams it within the allocation",
            )
            .with_fix_hint("chunks=\"auto\"")
            .with_url("https://github.com/greensh16/xray-cs/wiki/Job-Rules#job003"),
        );
    }
}

/// JOB004 — a GPU allocation nothing in the run can use.
///
/// The only run-level rule in xray. Two things make it so:
///
///   * It reports against the *job script*, not the Python — there is no line
///     of Python to point at, and the actionable edit is the directive. Fired
///     per file it would repeat one mistake once per file linted.
///   * The question is "does anything this job launches touch the GPU?", which
///     no single file can answer. A package where `model.py` imports torch and
///     `utils.py` does not is using its GPU perfectly well.
///
/// So it takes the whole run's import picture and emits at most one
/// diagnostic. Called from `runner::run`, not from `rules::run_all_with_job`.
pub fn job004(any_file_imports_gpu: bool, job: &JobScript, config: &Config) -> Option<Diagnostic> {
    if config.is_disabled("JOB004") || any_file_imports_gpu || !job.has_directives {
        return None;
    }
    let gpu = job.gpu.as_ref()?;
    Some(
        Diagnostic::new(
            "JOB004",
            Severity::Warning,
            job.path.clone(),
            gpu.line,
            1,
            format!(
                "`{}` requests a GPU, but the Python it launches imports no GPU library — the device is allocated, billed, and never touched",
                gpu.raw
            ),
        )
        .with_suggestion(
            "Drop the GPU request and submit to a CPU queue, or port the hot path to CuPy / PyTorch if the GPU was the point",
        )
        .with_url("https://github.com/greensh16/xray-cs/wiki/Job-Rules#job004"),
    )
}

/// JOB005 — an unbounded pool under a partial-node allocation.
fn job005(
    facts: &PythonFacts,
    job: &JobScript,
    path: &str,
    config: &Config,
    out: &mut Vec<Diagnostic>,
) {
    if config.is_disabled("JOB005") {
        return;
    }
    // `--exclusive` means the job owns the node, so taking every core on it is
    // exactly right. Only a *partial* allocation makes this a problem.
    let Some(cpus) = job.cpus.as_ref() else {
        return;
    };
    if job.exclusive {
        return;
    }

    for (line, column, what) in &facts.unbounded_pools {
        out.push(
            Diagnostic::new(
                "JOB005",
                Severity::Warning,
                path,
                *line,
                *column,
                format!(
                    "{what} sizes itself from the machine's core count, not the job's — {} allocates {} core{} on a shared node, so this takes cores belonging to other jobs",
                    job.path,
                    cpus.value,
                    if cpus.value == 1 { "" } else { "s" }
                ),
            )
            .with_suggestion(format!(
                "Read the allocation instead: `n_jobs=int(os.environ.get(\"SLURM_CPUS_PER_TASK\", 1))`, or hard-code {} to match the request",
                cpus.value
            ))
            .with_url("https://github.com/greensh16/xray-cs/wiki/Job-Rules#job005"),
        );
    }
}

/// One query pass over the Python file, gathering everything the rules above
/// compare against the scheduler directives.
fn collect_facts(file: &ParsedFile) -> PythonFacts {
    let mut facts = PythonFacts::default();
    let source = file.source.as_bytes();
    let query = &*QUERY;
    let mut cursor = QueryCursor::new();

    let mut matches = cursor.matches(query, file.tree.root_node(), source);
    while let Some(m) = matches.next() {
        match m.pattern_index {
            // A dask cluster construction.
            0 => {
                let Some(call) = capture(query, m, "job_cluster_call") else {
                    continue;
                };
                let name = callee_name(call, source).unwrap_or("cluster").to_string();
                // `Client(existing_cluster)` forwards to a cluster built
                // elsewhere and sets no worker count of its own.
                let n_workers = int_kwarg(call, source, "n_workers");
                let threads_set = has_keyword_arg(call, source, "threads_per_worker");
                if n_workers.is_none() && !threads_set {
                    continue;
                }
                let (line, column) = position(&call);
                // The first cluster in the file wins: a script that builds two
                // is doing something xray cannot reason about.
                facts.cluster.get_or_insert(ClusterFact {
                    line,
                    column,
                    name,
                    n_workers,
                    threads_per_worker: int_kwarg(call, source, "threads_per_worker"),
                    threads_set,
                });
            }

            // An open_dataset / open_mfdataset left unchunked.
            1 => {
                let Some(call) = capture(query, m, "job_open_call") else {
                    continue;
                };
                let Some(name) = callee_name(call, source) else {
                    continue;
                };
                if !matches!(name, "open_dataset" | "open_mfdataset") {
                    continue;
                }
                if keyword_arg_present_or_unknown(call, source, "chunks") {
                    continue;
                }
                let (line, column) = position(&call);
                facts.unchunked_opens.push((line, column, name.to_string()));
            }

            // `n_jobs=-1` / `max_workers=-1`.
            2 => {
                let Some(kwarg) = capture(query, m, "job_njobs_kwarg") else {
                    continue;
                };
                let Some(value) = capture(query, m, "job_njobs_value") else {
                    continue;
                };
                if node_text(&value, source) != "1" {
                    continue;
                }
                let Some(name) = kwarg
                    .child_by_field_name("name")
                    .map(|n| node_text(&n, source))
                else {
                    continue;
                };
                let (line, column) = position(&kwarg);
                facts
                    .unbounded_pools
                    .push((line, column, format!("`{name}=-1`")));
            }

            // `Pool()` / `ProcessPoolExecutor()` with no worker count.
            3 => {
                let Some(call) = capture(query, m, "job_pool_call") else {
                    continue;
                };
                let Some(name) = callee_name(call, source) else {
                    continue;
                };
                if !matches!(name, "Pool" | "ProcessPoolExecutor" | "ThreadPoolExecutor") {
                    continue;
                }
                // A worker count in any form — positional or keyword — means
                // the author chose one, and xray is not auditing the number.
                let args = call.child_by_field_name("arguments");
                let has_any_arg = args.is_some_and(|a| a.named_child_count() > 0);
                if has_any_arg {
                    continue;
                }
                let (line, column) = position(&call);
                facts.unbounded_pools.push((
                    line,
                    column,
                    format!("`{name}()` with no worker count"),
                ));
            }

            _ => {}
        }
    }

    facts
}

fn capture<'t>(query: &Query, m: &tree_sitter::QueryMatch<'_, 't>, name: &str) -> Option<Node<'t>> {
    query
        .capture_index_for_name(name)
        .and_then(|i| m.nodes_for_capture_index(i).next())
}

/// The name of the function being called: the attribute for `a.b()`, the bare
/// identifier for `b()`.
fn callee_name<'a>(call: Node<'_>, source: &'a [u8]) -> Option<&'a str> {
    let func = call.child_by_field_name("function")?;
    let name_node = match func.kind() {
        "attribute" => func.child_by_field_name("attribute")?,
        "identifier" => func,
        _ => return None,
    };
    Some(node_text(&name_node, source))
}

/// The value of an integer-literal keyword argument. `None` covers both
/// "absent" and "not a literal xray can read" — both mean stay quiet.
fn int_kwarg(call: Node<'_>, source: &[u8], kw: &str) -> Option<usize> {
    keyword_arg_value(call, source, kw)?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::job::parse_job_source;
    use crate::parser::parse_source;

    fn check(script: &str, python: &str) -> Vec<Diagnostic> {
        let job = parse_job_source(script, "run.sh");
        let parsed = parse_source(python.to_string()).unwrap();
        JobRules::check(&parsed, "analysis.py", &Config::default(), &job)
    }

    fn ids(script: &str, python: &str) -> Vec<&'static str> {
        let mut d = check(script, python);
        d.sort_by_key(|d| (d.line, d.rule_id));
        d.into_iter().map(|d| d.rule_id).collect()
    }

    fn fires(rule: &str, script: &str, python: &str) -> bool {
        ids(script, python).contains(&rule)
    }

    const SLURM_48: &str = "#!/bin/bash\n#SBATCH --cpus-per-task=48\nexport OMP_NUM_THREADS=1\n";

    #[test]
    fn job001_flags_a_cluster_smaller_than_the_allocation() {
        assert!(fires(
            "JOB001",
            SLURM_48,
            "from dask.distributed import LocalCluster\ncluster = LocalCluster(n_workers=4)\n",
        ));
        // 48 workers × 1 thread matches 48 cores exactly.
        assert!(!fires(
            "JOB001",
            SLURM_48,
            "from dask.distributed import LocalCluster\ncluster = LocalCluster(n_workers=48, threads_per_worker=1)\n",
        ));
        // 12 × 4 also matches.
        assert!(!fires(
            "JOB001",
            SLURM_48,
            "from dask.distributed import LocalCluster\ncluster = LocalCluster(n_workers=12, threads_per_worker=4)\n",
        ));
    }

    #[test]
    fn job001_stays_quiet_when_either_side_is_unreadable() {
        // A shell variable is not a core count xray can check against.
        assert!(!fires(
            "JOB001",
            "#SBATCH --cpus-per-task=$NCPUS\n",
            "from dask.distributed import LocalCluster\ncluster = LocalCluster(n_workers=4)\n",
        ));
        // Nor is a computed worker count.
        assert!(!fires(
            "JOB001",
            SLURM_48,
            "from dask.distributed import LocalCluster\ncluster = LocalCluster(n_workers=int(os.environ['N']))\n",
        ));
    }

    #[test]
    fn job002_needs_both_levers_missing() {
        let no_env = "#SBATCH --cpus-per-task=48\n";
        assert!(fires(
            "JOB002",
            no_env,
            "from dask.distributed import LocalCluster\nc = LocalCluster(n_workers=48)\n",
        ));
        // The job script pins threads.
        assert!(!fires(
            "JOB002",
            SLURM_48,
            "from dask.distributed import LocalCluster\nc = LocalCluster(n_workers=48)\n",
        ));
        // The Python pins threads.
        assert!(!fires(
            "JOB002",
            no_env,
            "from dask.distributed import LocalCluster\nc = LocalCluster(n_workers=48, threads_per_worker=1)\n",
        ));
        // A single-core request cannot oversubscribe.
        assert!(!fires(
            "JOB002",
            "#SBATCH --cpus-per-task=1\n",
            "from dask.distributed import LocalCluster\nc = LocalCluster(n_workers=1)\n",
        ));
    }

    #[test]
    fn job003_pairs_a_memory_request_with_an_unchunked_read() {
        let script = "#SBATCH --mem=190GB\n";
        assert!(fires(
            "JOB003",
            script,
            "import xarray as xr\nds = xr.open_mfdataset('era5_*.nc')\n",
        ));
        assert!(!fires(
            "JOB003",
            script,
            "import xarray as xr\nds = xr.open_mfdataset('era5_*.nc', chunks='auto')\n",
        ));
        // No memory request, nothing to pair against.
        assert!(!fires(
            "JOB003",
            "#SBATCH --cpus-per-task=4\n",
            "import xarray as xr\nds = xr.open_mfdataset('era5_*.nc')\n",
        ));
    }

    #[test]
    fn job004_flags_a_gpu_request_nothing_in_the_run_can_use() {
        let gpu = parse_job_source("#SBATCH --gres=gpu:v100:2\n", "run.sh");
        let cfg = Config::default();
        assert!(job004(false, &gpu, &cfg).is_some());
        // One file anywhere in the run touching the GPU is enough.
        assert!(job004(true, &gpu, &cfg).is_none());
        // No GPU asked for, nothing to report.
        let cpu = parse_job_source("#SBATCH --cpus-per-task=4\n", "run.sh");
        assert!(job004(false, &cpu, &cfg).is_none());
    }

    #[test]
    fn job004_reports_against_the_job_script_not_the_python() {
        let job = parse_job_source("#SBATCH --gres=gpu:1\n", "run.sh");
        let d = job004(false, &job, &Config::default()).unwrap();
        assert_eq!(d.file, "run.sh");
        assert_eq!(d.line, 1);
    }

    #[test]
    fn job004_never_comes_from_the_per_file_pass() {
        // Otherwise a 50-file run reports one job-script mistake 50 times.
        assert!(!fires(
            "JOB004",
            "#SBATCH --gres=gpu:v100:2\n",
            "import numpy as np\nx = np.zeros(4)\n"
        ));
    }

    #[test]
    fn job005_flags_unbounded_pools_on_a_partial_node() {
        let script = "#SBATCH --cpus-per-task=4\n";
        assert!(fires(
            "JOB005",
            script,
            "from joblib import Parallel\nParallel(n_jobs=-1)(tasks)\n",
        ));
        assert!(fires(
            "JOB005",
            script,
            "from multiprocessing import Pool\np = Pool()\n",
        ));
        // A chosen worker count is a decision, not a grab.
        assert!(!fires(
            "JOB005",
            script,
            "from multiprocessing import Pool\np = Pool(4)\n",
        ));
        assert!(!fires(
            "JOB005",
            script,
            "from joblib import Parallel\nParallel(n_jobs=4)(tasks)\n",
        ));
    }

    #[test]
    fn job005_allows_an_exclusive_node() {
        // With the whole node, taking every core on it is correct.
        assert!(!fires(
            "JOB005",
            "#SBATCH --cpus-per-task=48\n#SBATCH --exclusive\n",
            "from joblib import Parallel\nParallel(n_jobs=-1)(tasks)\n",
        ));
    }

    #[test]
    fn a_script_with_no_directives_produces_nothing() {
        assert!(
            check(
                "#!/bin/bash\npython analysis.py\n",
                "import xarray as xr\nds = xr.open_mfdataset('a*.nc')\n",
            )
            .is_empty()
        );
    }

    #[test]
    fn disabled_job_rules_do_not_fire() {
        let job = parse_job_source(SLURM_48, "run.sh");
        let parsed = parse_source(
            "from dask.distributed import LocalCluster\nc = LocalCluster(n_workers=4)\n"
                .to_string(),
        )
        .unwrap();
        let mut config = Config::default();
        config.disable.insert("JOB001".to_string());
        let diags = JobRules::check(&parsed, "analysis.py", &config, &job);
        assert!(diags.iter().all(|d| d.rule_id != "JOB001"));
    }
}
