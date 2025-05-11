async function createOne(projectId: number) {
    const req = await fetch("http://localhost:8080/task", {
        method: "POST",
        headers: {
            "content-type": "application/json",
        },
        body: JSON.stringify({
            "name": "cluster",
            "kind": "cluster",
            "timeout": 6,
            "rules": {
                // condition to before starting the task
                "conditions": [
                    {
                        type: "Concurency",
                        max_concurency: 1, // 1 "cluster" at a given time on a given project
                        matcher: {
                            status: "Running",
                            kind: "cluster",
                            // if field `projectId` is present then we match
                            // Running && cluster && projectId == metadata.projectId
                            // else just
                            // Running && cluster
                            fields: [
                                "projectId",
                            ],
                        },
                    },
                    {
                        type: "Concurency",
                        max_concurency: 4, // 4 "cluster" at a given time
                        matcher: {
                            status: "Running",
                            kind: "cluster",
                            fields: [],
                        },
                    },
                ],
            },
            "metadata": {
                "projectId": projectId,
            },
            "actions": [
                {
                    "trigger": "Start", // action executed on start
                    "name": "Call main api", // could be removed
                    "kind": "Webhook",
                    "params": {
                        "url": "http://localhost:9090/task",
                        "verb": "Post",
                        "body": {
                            "projectId": projectId, // could use a ref to the metadata ?
                            "wait_for": 2,
                        },
                        "headers": {
                            "content-type": "application/json",
                        },
                    },
                },
            ],
        }),
    });

    const text = await req.text();
    console.log(text);
    const js = JSON.parse(text);
    console.log(js);
    console.log(js.actions);
    console.log(js.rules);
}

async function main() {
    await createOne(1251);
    await createOne(1253);
}

main();
