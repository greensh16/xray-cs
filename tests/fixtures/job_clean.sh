#!/bin/bash
#SBATCH --job-name=era5
#SBATCH --cpus-per-task=48
#SBATCH --mem=190GB
#SBATCH --time=04:00:00

export OMP_NUM_THREADS=1
export MKL_NUM_THREADS=1

module load python3/3.11
python3 job_clean.py
