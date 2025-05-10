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
                        max_concurency: 1,
                        matcher: {
                            status: "Running",
                            kind: "cluster",
                            // if field `projectId` is present then we match 
                            // Running && cluster && projectId == metadata.projectId
                            // else just
                            // Running && cluster
                            fields: [
                                // "projectId",
                            ],
                        },
                    },
                ],
            },
            "metadata": {
                "projectId": projectId,
            },
            "actions": [
                {
                    // add trigger
                    "name": "Call main api", // could be removed
                    "kind": "Webhook",
                    "params": {
                        "url": "http://localhost:9090/task",
                        "verb": "Post",
                        "body": {
                            "values_in_body": [1, 4],
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