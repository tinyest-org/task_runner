const BASE_URL = "http://localhost:8085";

// Create a single task (no dependencies)
async function createSingleTask(projectId: number) {
    const req = await fetch(`${BASE_URL}/task`, {
        method: "POST",
        headers: {
            "requester": "tester/Bun",
            "content-type": "application/json",
        },
        body: JSON.stringify([{
            "id": "task-1", // local ID for dependency resolution
            "name": "cluster",
            "kind": "cluster",
            "timeout": 60,
            "dedupe_strategy": [
                {
                    status: "Pending",
                    kind: "cluster",
                    fields: ["projectId"],
                },
            ],
            "rules": {
                "conditions": [
                    {
                        type: "Concurency",
                        max_concurency: 1,
                        matcher: {
                            status: "Running",
                            kind: "cluster",
                            fields: ["projectId"],
                        },
                    },
                ],
            },
            "metadata": {
                "projectId": projectId,
            },
            "on_start": {
                "trigger": "Start",
                "kind": "Webhook",
                "params": {
                    "url": "https://httpbin.org/post",
                    "verb": "Post",
                    "body": {
                        "projectId": projectId,
                        "action": "start",
                    },
                    "headers": {
                        "content-type": "application/json",
                    },
                },
            },
            "on_success": [
                {
                    "trigger": "End",
                    "kind": "Webhook",
                    "params": {
                        "url": "https://httpbin.org/post",
                        "verb": "Post",
                        "body": { "status": "success" },
                        "headers": { "content-type": "application/json" },
                    },
                },
            ],
            "on_failure": [
                {
                    "trigger": "End",
                    "kind": "Webhook",
                    "params": {
                        "url": "https://httpbin.org/post",
                        "verb": "Post",
                        "body": { "status": "failure" },
                        "headers": { "content-type": "application/json" },
                    },
                },
            ],
        }]),
    });

    const text = await req.text();
    console.log("Response:", text);
    return JSON.parse(text);
}

// Create a DAG of tasks with dependencies
async function createDag() {
    const tasks = [
        // Root task (no dependencies)
        {
            "id": "root",
            "name": "Build Project",
            "kind": "build",
            "timeout": 120,
            "metadata": { "step": "build" },
            "on_start": {
                "trigger": "Start",
                "kind": "Webhook",
                "params": {
                    "url": "https://httpbin.org/delay/2",
                    "verb": "Post",
                    "body": { "task": "build" },
                    "headers": { "content-type": "application/json" },
                },
            },
        },
        // Two parallel tasks depending on root
        {
            "id": "test-unit",
            "name": "Unit Tests",
            "kind": "test",
            "timeout": 60,
            "metadata": { "step": "test", "type": "unit" },
            "dependencies": [
                { "id": "root", "requires_success": true }
            ],
            "on_start": {
                "trigger": "Start",
                "kind": "Webhook",
                "params": {
                    "url": "https://httpbin.org/delay/1",
                    "verb": "Post",
                    "body": { "task": "unit-tests" },
                    "headers": { "content-type": "application/json" },
                },
            },
        },
        {
            "id": "test-integration",
            "name": "Integration Tests",
            "kind": "test",
            "timeout": 90,
            "metadata": { "step": "test", "type": "integration" },
            "dependencies": [
                { "id": "root", "requires_success": true }
            ],
            "on_start": {
                "trigger": "Start",
                "kind": "Webhook",
                "params": {
                    "url": "https://httpbin.org/delay/3",
                    "verb": "Post",
                    "body": { "task": "integration-tests" },
                    "headers": { "content-type": "application/json" },
                },
            },
        },
        // Lint task also depends on root but doesn't require success
        {
            "id": "lint",
            "name": "Lint Check",
            "kind": "lint",
            "timeout": 30,
            "metadata": { "step": "lint" },
            "dependencies": [
                { "id": "root", "requires_success": false }
            ],
            "on_start": {
                "trigger": "Start",
                "kind": "Webhook",
                "params": {
                    "url": "https://httpbin.org/delay/1",
                    "verb": "Post",
                    "body": { "task": "lint" },
                    "headers": { "content-type": "application/json" },
                },
            },
        },
        // Deploy depends on both test tasks succeeding
        {
            "id": "deploy",
            "name": "Deploy to Staging",
            "kind": "deploy",
            "timeout": 180,
            "metadata": { "step": "deploy", "env": "staging" },
            "dependencies": [
                { "id": "test-unit", "requires_success": true },
                { "id": "test-integration", "requires_success": true },
                { "id": "lint", "requires_success": false }
            ],
            "on_start": {
                "trigger": "Start",
                "kind": "Webhook",
                "params": {
                    "url": "https://httpbin.org/delay/2",
                    "verb": "Post",
                    "body": { "task": "deploy" },
                    "headers": { "content-type": "application/json" },
                },
            },
            "on_success": [
                {
                    "trigger": "End",
                    "kind": "Webhook",
                    "params": {
                        "url": "https://httpbin.org/post",
                        "verb": "Post",
                        "body": { "deployed": true },
                        "headers": { "content-type": "application/json" },
                    },
                },
            ],
        },
    ];

    const req = await fetch(`${BASE_URL}/task`, {
        method: "POST",
        headers: {
            "requester": "tester/Bun",
            "content-type": "application/json",
        },
        body: JSON.stringify(tasks),
    });

    const batchId = req.headers.get("X-Batch-ID");
    const text = await req.text();
    const result = JSON.parse(text);

    console.log("Created DAG with batch_id:", batchId);
    console.log("Tasks created:", result.length);
    console.log("\nView DAG at:", `${BASE_URL}/view?batch=${batchId}`);
    console.log("\nTasks:");
    result.forEach((t: any) => {
        console.log(`  - ${t.name} (${t.id}) - ${t.status}`);
    });

    return { batchId, tasks: result };
}

// Update a task status
async function updateTask(taskId: string, status: "Success" | "Failure") {
    const req = await fetch(`${BASE_URL}/task/${taskId}`, {
        method: "PATCH",
        headers: {
            "content-type": "application/json",
        },
        body: JSON.stringify({
            "status": status,
        }),
    });
    console.log(`Updated task ${taskId} to ${status}:`, req.status);
}

// Get DAG data
async function getDag(batchId: string) {
    const req = await fetch(`${BASE_URL}/dag/${batchId}`);
    const data = await req.json();
    console.log("DAG data:", JSON.stringify(data, null, 2));
    return data;
}

// List tasks
async function listTasks() {
    const req = await fetch(`${BASE_URL}/task`);
    const tasks = await req.json();
    console.log("Tasks:", tasks.length);
    tasks.forEach((t: any) => {
        console.log(`  [${t.status}] ${t.name} (${t.id}) batch=${t.batch_id}`);
    });
    return tasks;
}

async function main() {
    const args = process.argv.slice(2);
    const command = args[0] || "dag";

    switch (command) {
        case "single":
            await createSingleTask(1251);
            break;
        case "dag":
            await createDag();
            break;
        case "list":
            await listTasks();
            break;
        case "update":
            if (args.length < 3) {
                console.log("Usage: bun test.ts update <task_id> <Success|Failure>");
                return;
            }
            await updateTask(args[1], args[2] as "Success" | "Failure");
            break;
        case "view":
            if (args.length < 2) {
                console.log("Usage: bun test.ts view <batch_id>");
                return;
            }
            await getDag(args[1]);
            break;
        default:
            console.log("Commands: single, dag, list, update, view");
    }
}

main();
