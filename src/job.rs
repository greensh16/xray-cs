//! Scheduler directive parsing for `xray --job`.
//!
//! Every other Python linter stops at the file boundary. On HPC the resource
//! request is where jobs are most often wrong, and it sits in a submission
//! script that no Python tool ever reads — so a job asking for 48 cores and
//! building a 4-worker cluster looks perfectly clean to both halves in
//! isolation.
//!
//! Reading it stays inside xray's purely-syntactic remit. `#SBATCH` and `#PBS`
//! directives are line-oriented comments with a fixed option grammar; nothing
//! here executes the shell, expands a variable, or follows an include. A
//! directive xray cannot parse becomes `None`, and every JOB rule treats
//! `None` as "say nothing" — the same invariant the binding tracker follows.

use anyhow::{Context, Result};
use std::path::Path;

/// A parsed submission script: the resource request, and the few shell
/// assignments that bear on how the Python inside it will behave.
#[derive(Debug, Default, Clone)]
pub struct JobScript {
    /// Path as the user gave it, for diagnostics that point back at a directive.
    pub path: String,
    /// Cores requested for one task — Slurm `--cpus-per-task` / `-c`, PBS
    /// `-l ncpus=`.
    pub cpus: Option<Directive<usize>>,
    /// Memory requested, normalised to bytes. Slurm `--mem`, PBS `-l mem=`.
    /// `--mem-per-cpu` is multiplied out when the core count is also known.
    pub memory_bytes: Option<Directive<u64>>,
    /// A GPU was asked for: `--gres=gpu:...`, `--gpus=`, `-l ngpus=`, or a
    /// queue/partition whose name says so.
    pub gpu: Option<Directive<String>>,
    /// `OMP_NUM_THREADS` / `MKL_NUM_THREADS` / `OPENBLAS_NUM_THREADS` and
    /// friends are set somewhere in the script.
    pub thread_env: Option<Directive<String>>,
    /// `--exclusive` — the job owns the whole node, so grabbing every core on
    /// it is legitimate.
    pub exclusive: bool,
    /// True when at least one `#SBATCH` or `#PBS` line was found. A shell
    /// script with no directives at all is almost certainly not the submission
    /// script the user meant to pass.
    pub has_directives: bool,
    /// Slurm's `--mem-per-cpu`, held per core until the core count is known.
    /// Private: it is an intermediate, and `memory_bytes` is the total every
    /// rule should read.
    mem_per_cpu: Option<Directive<u64>>,
}

/// A value together with the 1-based line it was read from, so a diagnostic
/// can point at the directive that produced it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Directive<T> {
    pub value: T,
    pub line: usize,
    /// The directive text as written, for the diagnostic message.
    pub raw: String,
}

impl<T> Directive<T> {
    fn new(value: T, line: usize, raw: &str) -> Self {
        Self {
            value,
            line,
            raw: raw.trim().to_string(),
        }
    }
}

/// Environment variables that cap the thread pool of a BLAS/OpenMP runtime.
///
/// Setting any one of them is taken as evidence the author thought about
/// oversubscription; JOB002 does not audit whether the number is right.
const THREAD_ENV_VARS: &[&str] = &[
    "OMP_NUM_THREADS",
    "MKL_NUM_THREADS",
    "OPENBLAS_NUM_THREADS",
    "NUMEXPR_NUM_THREADS",
    "VECLIB_MAXIMUM_THREADS",
    "BLIS_NUM_THREADS",
];

pub fn parse_job_file(path: &str) -> Result<JobScript> {
    let bytes = std::fs::read(path).with_context(|| format!("cannot read job script {path}"))?;
    let source = String::from_utf8_lossy(&bytes).into_owned();
    Ok(parse_job_source(&source, path))
}

/// Parse a submission script's text. Split out from [`parse_job_file`] so the
/// tests need no fixtures on disk.
pub fn parse_job_source(source: &str, path: &str) -> JobScript {
    let mut job = JobScript {
        path: path.to_string(),
        ..Default::default()
    };

    for (idx, raw_line) in source.replace("\r\n", "\n").lines().enumerate() {
        let line_no = idx + 1;
        let line = raw_line.trim();

        if let Some(rest) = directive_body(line, "#SBATCH") {
            job.has_directives = true;
            parse_sbatch(&mut job, rest, line_no, raw_line);
        } else if let Some(rest) = directive_body(line, "#PBS") {
            job.has_directives = true;
            parse_pbs(&mut job, rest, line_no, raw_line);
        } else if !line.starts_with('#') {
            // Only non-comment lines can export anything.
            parse_thread_env(&mut job, line, line_no, raw_line);
        }
    }

    // `--mem-per-cpu` is only meaningful once the core count is known; it is
    // stashed as a per-core figure and multiplied out here.
    if let (Some(per_cpu), Some(cpus)) = (job.mem_per_cpu.take(), job.cpus.as_ref()) {
        job.memory_bytes = Some(Directive {
            value: per_cpu.value.saturating_mul(cpus.value as u64),
            line: per_cpu.line,
            raw: per_cpu.raw,
        });
    }

    job
}

/// The text after a scheduler directive prefix, if this line is one.
///
/// A prefix match alone is not enough: `#SBATCHELOR` is a comment, and
/// `#SBATCH` inside a heredoc is out of xray's reach either way.
fn directive_body<'a>(line: &'a str, prefix: &str) -> Option<&'a str> {
    let rest = line.strip_prefix(prefix)?;
    if rest.is_empty() || rest.starts_with([' ', '\t']) {
        Some(rest.trim())
    } else {
        None
    }
}

fn parse_sbatch(job: &mut JobScript, body: &str, line: usize, raw: &str) {
    let tokens: Vec<&str> = body.split_whitespace().collect();
    let mut i = 0;
    while i < tokens.len() {
        let token = tokens[i];
        // Both `--opt=value` and `--opt value` / `-c value` are legal.
        let (key, inline_value) = match token.split_once('=') {
            Some((k, v)) => (k, Some(v)),
            None => (token, None),
        };
        let value = inline_value.or_else(|| tokens.get(i + 1).copied());

        match key {
            "--cpus-per-task" | "-c" => {
                if let Some(n) = value.and_then(|v| v.parse::<usize>().ok()) {
                    job.cpus = Some(Directive::new(n, line, raw));
                }
            }
            "--mem" => {
                if let Some(b) = value.and_then(parse_size) {
                    job.memory_bytes = Some(Directive::new(b, line, raw));
                }
            }
            "--mem-per-cpu" => {
                if let Some(b) = value.and_then(parse_size) {
                    job.mem_per_cpu = Some(Directive::new(b, line, raw));
                }
            }
            "--gres" => {
                if value.is_some_and(|v| v.starts_with("gpu")) {
                    job.gpu = Some(Directive::new(value.unwrap().to_string(), line, raw));
                }
            }
            "--gpus" | "--gpus-per-node" | "--gpus-per-task" | "-G" => {
                if let Some(v) = value {
                    job.gpu = Some(Directive::new(v.to_string(), line, raw));
                }
            }
            "--partition" | "-p" => {
                if let Some(v) = value
                    && looks_like_gpu_queue(v)
                {
                    job.gpu = Some(Directive::new(v.to_string(), line, raw));
                }
            }
            "--exclusive" => job.exclusive = true,
            _ => {}
        }
        i += 1;
    }
}

fn parse_pbs(job: &mut JobScript, body: &str, line: usize, raw: &str) {
    let tokens: Vec<&str> = body.split_whitespace().collect();
    let mut i = 0;
    while i < tokens.len() {
        match tokens[i] {
            // `#PBS -l ncpus=48,mem=190GB,ngpus=4` — one comma-separated list.
            "-l" => {
                if let Some(spec) = tokens.get(i + 1) {
                    for item in spec.split(',') {
                        let Some((key, value)) = item.split_once('=') else {
                            continue;
                        };
                        match key.trim() {
                            "ncpus" => {
                                if let Ok(n) = value.trim().parse::<usize>() {
                                    job.cpus = Some(Directive::new(n, line, raw));
                                }
                            }
                            "mem" => {
                                if let Some(b) = parse_size(value.trim()) {
                                    job.memory_bytes = Some(Directive::new(b, line, raw));
                                }
                            }
                            "ngpus" => {
                                // `ngpus=0` is a request for none.
                                if value.trim().parse::<usize>().is_ok_and(|n| n > 0) {
                                    job.gpu =
                                        Some(Directive::new(value.trim().to_string(), line, raw));
                                }
                            }
                            _ => {}
                        }
                    }
                }
                i += 1;
            }
            // `#PBS -q gpuvolta`
            "-q" => {
                if let Some(q) = tokens.get(i + 1)
                    && looks_like_gpu_queue(q)
                {
                    job.gpu = Some(Directive::new((*q).to_string(), line, raw));
                }
                i += 1;
            }
            _ => {}
        }
        i += 1;
    }
}

/// `export OMP_NUM_THREADS=8`, `OMP_NUM_THREADS=8`, and the `setenv` spelling.
fn parse_thread_env(job: &mut JobScript, line: &str, line_no: usize, raw: &str) {
    if job.thread_env.is_some() {
        return;
    }
    let stripped = line
        .strip_prefix("export ")
        .or_else(|| line.strip_prefix("setenv "))
        .unwrap_or(line)
        .trim();

    for var in THREAD_ENV_VARS {
        // `setenv VAR 8` uses a space where the others use `=`.
        let matches = stripped
            .strip_prefix(var)
            .is_some_and(|rest| rest.starts_with('=') || rest.starts_with(' '));
        if matches {
            job.thread_env = Some(Directive::new((*var).to_string(), line_no, raw));
            return;
        }
    }
}

/// Queue and partition names that mean a GPU across the common schedulers.
fn looks_like_gpu_queue(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.contains("gpu") || lower.contains("cuda") || lower.contains("dgx")
}

/// `"64G"`, `"190GB"`, `"4000MB"`, `"512"` → bytes.
///
/// A bare number is megabytes, which is Slurm's documented default for
/// `--mem`. Anything else — `$MEM`, `lots` — is `None`, and the rules stay
/// quiet rather than guess.
fn parse_size(text: &str) -> Option<u64> {
    let text = text.trim();
    let digits_end = text
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(text.len());
    if digits_end == 0 {
        return None;
    }
    let (number, suffix) = text.split_at(digits_end);
    let n: u64 = number.parse().ok()?;
    let multiplier = match suffix.trim().to_ascii_uppercase().as_str() {
        "" | "M" | "MB" => 1024 * 1024,
        "K" | "KB" => 1024,
        "G" | "GB" => 1024 * 1024 * 1024,
        "T" | "TB" => 1024_u64.pow(4),
        _ => return None,
    };
    n.checked_mul(multiplier)
}

/// Human-readable memory, for diagnostic messages.
pub fn format_bytes(bytes: u64) -> String {
    const GB: u64 = 1024 * 1024 * 1024;
    const MB: u64 = 1024 * 1024;
    if bytes >= GB {
        format!("{:.0} GB", bytes as f64 / GB as f64)
    } else {
        format!("{:.0} MB", bytes as f64 / MB as f64)
    }
}

/// Locate the submission script for a run: `--job` if given, else the first
/// match of `[job].script` walking the configured glob.
pub fn resolve_job_script(
    explicit: Option<&str>,
    glob_pattern: Option<&str>,
) -> Result<Option<JobScript>> {
    if let Some(path) = explicit {
        if !Path::new(path).exists() {
            anyhow::bail!("job script not found: {path}");
        }
        return Ok(Some(parse_job_file(path)?));
    }
    let Some(pattern) = glob_pattern else {
        return Ok(None);
    };
    let mut matches: Vec<String> = glob::glob(pattern)
        .with_context(|| format!("invalid `[job].script` glob: {pattern}"))?
        .filter_map(|e| e.ok())
        .filter(|p| p.is_file())
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    // Deterministic across filesystems: a glob's order is not specified, and a
    // lint run that picks a different script per machine is worse than none.
    matches.sort();
    match matches.first() {
        Some(path) => Ok(Some(parse_job_file(path)?)),
        None => Ok(None),
    }
}

impl JobScript {
    /// True when the script asked for more than one core.
    pub fn is_multicore(&self) -> bool {
        self.cpus.as_ref().is_some_and(|c| c.value > 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(src: &str) -> JobScript {
        parse_job_source(src, "run.sh")
    }

    #[test]
    fn slurm_directives_in_both_spellings() {
        let job = parse(
            "#!/bin/bash\n\
             #SBATCH --cpus-per-task=48\n\
             #SBATCH --mem 190GB\n\
             #SBATCH --gres=gpu:v100:2\n",
        );
        assert_eq!(job.cpus.as_ref().unwrap().value, 48);
        assert_eq!(job.cpus.as_ref().unwrap().line, 2);
        assert_eq!(job.memory_bytes.unwrap().value, 190 * 1024 * 1024 * 1024);
        assert_eq!(job.gpu.unwrap().value, "gpu:v100:2");
        assert!(job.has_directives);
    }

    #[test]
    fn short_slurm_flags() {
        let job = parse("#SBATCH -c 16\n#SBATCH -p gpuvolta\n");
        assert_eq!(job.cpus.unwrap().value, 16);
        assert!(job.gpu.is_some());
    }

    #[test]
    fn pbs_resource_lists() {
        let job = parse("#PBS -l ncpus=48,mem=190GB,ngpus=4\n#PBS -q normal\n");
        assert_eq!(job.cpus.unwrap().value, 48);
        assert_eq!(job.memory_bytes.unwrap().value, 190 * 1024 * 1024 * 1024);
        assert!(job.gpu.is_some());
    }

    #[test]
    fn ngpus_zero_is_not_a_gpu_request() {
        let job = parse("#PBS -l ncpus=8,ngpus=0\n");
        assert!(job.gpu.is_none());
    }

    #[test]
    fn mem_per_cpu_multiplies_out_by_the_core_count() {
        let job = parse("#SBATCH --cpus-per-task=8\n#SBATCH --mem-per-cpu=4G\n");
        assert_eq!(job.memory_bytes.unwrap().value, 32 * 1024 * 1024 * 1024);
    }

    #[test]
    fn thread_env_is_found_in_every_spelling_but_not_in_a_comment() {
        assert!(parse("export OMP_NUM_THREADS=8\n").thread_env.is_some());
        assert!(parse("MKL_NUM_THREADS=4\n").thread_env.is_some());
        assert!(parse("setenv OMP_NUM_THREADS 8\n").thread_env.is_some());
        // A commented-out export sets nothing.
        assert!(parse("# export OMP_NUM_THREADS=8\n").thread_env.is_none());
    }

    #[test]
    fn a_prefix_that_only_looks_like_a_directive_is_a_comment() {
        let job = parse("#SBATCHELOR --cpus-per-task=48\n");
        assert!(!job.has_directives);
        assert!(job.cpus.is_none());
    }

    #[test]
    fn unparseable_values_leave_the_field_unset() {
        // `$NCPUS` needs a shell; xray does not have one, and guessing would
        // be worse than staying quiet.
        let job = parse("#SBATCH --cpus-per-task=$NCPUS\n#SBATCH --mem=lots\n");
        assert!(job.cpus.is_none());
        assert!(job.memory_bytes.is_none());
        assert!(job.has_directives);
    }

    #[test]
    fn memory_suffixes() {
        assert_eq!(parse_size("64G"), Some(64 * 1024 * 1024 * 1024));
        assert_eq!(parse_size("190GB"), Some(190 * 1024 * 1024 * 1024));
        assert_eq!(parse_size("4000MB"), Some(4000 * 1024 * 1024));
        // Slurm's bare number is megabytes.
        assert_eq!(parse_size("512"), Some(512 * 1024 * 1024));
        assert_eq!(parse_size("$MEM"), None);
        assert_eq!(parse_size("lots"), None);
    }

    #[test]
    fn exclusive_is_recorded() {
        assert!(parse("#SBATCH --exclusive\n").exclusive);
        assert!(!parse("#SBATCH --cpus-per-task=4\n").exclusive);
    }
}
