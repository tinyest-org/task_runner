const req = await fetch("http://localhost:8080/task", {
    method: "POST",
    headers: {
        "content-type": "application/json",
    },
    body: JSON.stringify({
        "name": "cluster",
        "kind": "cluster",
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
        "actions": [
            {
                "name": "Call main api",
                "kind": "Webhook",
                "params": {
                    "projectId": 1251,
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
