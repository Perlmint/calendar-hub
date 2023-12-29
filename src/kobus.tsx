import React from "react";
import { ActionFunctionArgs, Form, Navigate, Params, useLoaderData } from "react-router-dom";
import { AsyncReturnType, formDataToJsonString } from './utils';

export async function loader() {
    const resp = await fetch("/kobus/user", {
        credentials: "same-origin",
    });

    if (resp.ok) {
        const parsed = await resp.json();
        return {
            jsessionid: parsed.jsessionid as string,
        }
    } else {
        return null;
    }
}

export async function action({ request }: ActionFunctionArgs) {
    const formData = await request.formData();
    return await fetch("/kobus/user", {
        headers: {
            'Content-Type': 'application/json'
        },
        method: "POST",
        body: formDataToJsonString(formData)
    });
}

export function Component() {
    const data = useLoaderData() as AsyncReturnType<typeof loader>;

    if (data !== null) {
        return <div>
            <Form method="post" action="/kobus">
                <label htmlFor="ses">JSESSIONID</label>
                <input type="text" name="jsessionid" defaultValue={data.jsessionid} />
                <button type="submit">Update</button>
            </Form>
        </div>;
    } else {
        return <Navigate to="/" />;
    }
}
