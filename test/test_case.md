# Basic test case

- start task_runner
- start test server
- create with `concurrency: 1` using test script
- test server receives task does ACK
- task_runner updates the task with the `Running` status
- test server update success counter 4 times in 8 seconds
- test server validates end of task