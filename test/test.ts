const req = await fetch("http://localhost:8080/task", {
    method: "POST",
    headers: {
        "content-type": "application/json",
    },
    body: JSON.stringify({
        "name": "cluster",
        "kind": "cluster",
        "timeout": 4,
        "rules": {
            // condition to before starting the task
            "conditions": [
                {
                    type: "Concurency",
                    max_concurency: 1,
                    matcher: {
                        status: "Running",
                        kind: "clustering",
                        fields: [
                            "projectId",
                        ],
                    },
                },
            ],
        },
        "metadata": {
            "projectId": 1251,
        },
        "actions": [
            {
                // add trigger
                "name": "Call main api",
                "kind": "Webhook",
                "params": {
                    "url": "http://localhost:9090/task",
                    "verb": "Post",
                    "body": {
                        "values_in_body": [1, 4],
                    },
                    "headers": {
                        "content-type": "application/json"
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
