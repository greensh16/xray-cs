#!/bin/bash
#SBATCH --job-name=era5
#SBATCH --cpus-per-task=48
#SBATCH --mem=190GB
#SBATCH --gres=gpu:v100:2
#SBATCH --time=04:00:00

module load python3/3.11
python3 job_bad.py
