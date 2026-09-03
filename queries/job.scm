; Patterns for the Python half of the job-script cross-check (JOB001–JOB005).
;
; Unlike every other domain these do not map one-to-one onto rules: the JOB
; rules compare *facts about the file* against the scheduler directives, so
; this query gathers the facts and `src/rules/job.rs` decides what they mean
; once it has seen all of them.


; 0 — a dask cluster being constructed. LocalCluster / Client / and the
; distributed spellings all take n_workers= and threads_per_worker=.
(call
  function: [
    (attribute
      attribute: (identifier) @job_cluster_attr
      (#match? @job_cluster_attr "^(LocalCluster|Client|LocalCUDACluster|SLURMCluster|PBSCluster)$")
    )
    (identifier) @job_cluster_bare
    (#match? @job_cluster_bare "^(LocalCluster|Client|LocalCUDACluster|SLURMCluster|PBSCluster)$")
  ]
  arguments: (argument_list) @job_cluster_args
) @job_cluster_call


; 1 — a dataset being opened. JOB003 pairs a memory request against one of
; these left unchunked.
(call
  function: [
    (attribute
      attribute: (identifier) @job_open_attr
      (#match? @job_open_attr "^(open_dataset|open_mfdataset)$")
    )
    (identifier) @job_open_bare
    (#match? @job_open_bare "^(open_dataset|open_mfdataset)$")
  ]
  arguments: (argument_list) @job_open_args
) @job_open_call


; 2 — an unbounded worker pool: `n_jobs=-1` (joblib, scikit-learn) or
; `max_workers=-1`. The negative literal parses as a unary_operator.
(keyword_argument
  name: (identifier) @job_njobs_name
  value: (unary_operator
    argument: (integer) @job_njobs_value
  )
  (#match? @job_njobs_name "^(n_jobs|max_workers)$")
) @job_njobs_kwarg


; 3 — a process pool constructed with no worker count at all, which defaults
; to every core the machine has, not every core the job was given.
(call
  function: [
    (attribute
      attribute: (identifier) @job_pool_attr
      (#match? @job_pool_attr "^(Pool|ProcessPoolExecutor|ThreadPoolExecutor)$")
    )
    (identifier) @job_pool_bare
    (#match? @job_pool_bare "^(Pool|ProcessPoolExecutor|ThreadPoolExecutor)$")
  ]
  arguments: (argument_list) @job_pool_args
) @job_pool_call
