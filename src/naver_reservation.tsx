import React from "react";
import { ActionFunctionArgs, Form, Navigate, Params, useLoaderData } from "react-router-dom";
import { AsyncReturnType, formDataToJsonString } from './utils';

export async function loadData() {
    const resp = await fetch("/naver/user", {
        credentials: "same-origin",
    });

    if (resp.ok) {
        const parsed = await resp.json();
        return {
            ses: parsed.ses as string,
            aut: parsed.aut as string,
        }
    } else {
        return null;
    }
}

export async function updateAction({ request }: ActionFunctionArgs) {
    const formData = await request.formData();
    return await fetch("/naver/user", {
        headers: {
            'Content-Type': 'application/json'
        },
        body: formDataToJsonString(formData)
    });
}

export default function () {
    const data = useLoaderData() as AsyncReturnType<typeof loadData>;

    if (data !== null) {
        return <div>
            <Form method="post" action="/api/naver">
                <label htmlFor="ses">SES</label>
                <input type="text" name="ses" defaultValue={data.ses} />
                <label htmlFor="aut">AUT</label>
                <input type="text" name="aut" defaultValue={data.aut} />
                <button type="submit">Update</button>
            </Form>
        </div>;
    } else {
        return <Navigate to="/" />;
    }
}
