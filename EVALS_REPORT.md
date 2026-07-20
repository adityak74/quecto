⚠️  Warning: OPENROUTER_API_KEY is not set — LLM judge may fail.
    Finished `release` profile [optimized] target(s) in 0.10s
   Compiling quecto-agent v0.1.0 (/Users/adityakarnam/Projects/quecto/quecto-agent)
    Finished `release` profile [optimized] target(s) in 19.18s
QuECTO Smoke Eval Suite
Agent : qwen3.6:35b-mlx @ http://localhost:11434/v1
Judge : deterministic verify.sh

════════════════════════════════════════
 Task: tb_01_git_conflict_resolution
════════════════════════════════════════
  [setup] Initialized empty Git repository in /Users/adityakarnam/Projects/quecto/evals/results/workspace_tb_01_git_conflict_resolution/.git/
  [setup] [main (root-commit) f07de9c] init
  [setup]  1 file changed, 1 insertion(+)
  [setup]  create mode 100644 file.txt
  [setup] Switched to a new branch 'feature'
  [setup] [feature fb68305] feature
  [setup]  1 file changed, 1 insertion(+)
  [setup] Switched to branch 'main'
  [setup] [main 0de6d8d] main
  [setup]  1 file changed, 1 insertion(+)
  [setup] Auto-merging file.txt
  [setup] CONFLICT (content): Merge conflict in file.txt
  [setup] Automatic merge failed; fix conflicts and then commit the result.
--> Running quecto-agent...
--> Verifying (deterministic)...
Result: ✅  PASS

════════════════════════════════════════
 Task: tb_02_package_refactoring
════════════════════════════════════════
--> Running quecto-agent...
--> Verifying (deterministic)...
Result: ✅  PASS

════════════════════════════════════════
 Task: tb_03_advanced_sed_awk
════════════════════════════════════════
--> Running quecto-agent...
--> Verifying (deterministic)...
Result: ✅  PASS

════════════════════════════════════════
 Task: tb_04_openssl_decryption
════════════════════════════════════════
--> Running quecto-agent...
--> Verifying (deterministic)...
Result: ✅  PASS

════════════════════════════════════════
 Task: tb_05_dynamic_dependency_script
════════════════════════════════════════
--> Running quecto-agent...
--> Verifying (deterministic)...
Result: ✅  PASS

════════════════════════════════════════
 Task: tb_06_docker_build
════════════════════════════════════════
--> Running quecto-agent...
--> Verifying (deterministic)...
Result: ❌  FAIL

════════════════════════════════════════
 Task: tb_07_debug_c_crash
════════════════════════════════════════
--> Running quecto-agent...
--> Verifying (deterministic)...
Result: ✅  PASS

════════════════════════════════════════
 Task: tb_08_sqlite_query
════════════════════════════════════════
--> Running quecto-agent...
--> Verifying (deterministic)...
Result: ✅  PASS

════════════════════════════════════════
 Task: tb_09_fix_rust_build
════════════════════════════════════════
--> Running quecto-agent...
--> Verifying (deterministic)...
Result: ✅  PASS

════════════════════════════════════════
 Task: tb_10_openssl_selfsigned_cert
════════════════════════════════════════
--> Running quecto-agent...
--> Verifying (deterministic)...
Result: ✅  PASS

════════════════════════════════════════
 Results: 9/10 passed
════════════════════════════════════════
