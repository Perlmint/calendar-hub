import React from "react";
import { ActionFunctionArgs, Form, Navigate, Params, useLoaderData } from "react-router-dom";
import { AsyncReturnType, formDataToJsonString } from './utils';

export async function loader() {
    const resp = await fetch("/megabox/user", {
        credentials: "same-origin",
    });

    if (resp.ok) {
        const parsed = await resp.json();
        return {
            jsessionid: parsed.jsessionid as string,
            session: parsed.session as string,
        }
    } else {
        return null;
    }
}

export async function action({ request }: ActionFunctionArgs) {
    const formData = await request.formData();
    return await fetch("/megabox/user", {
        headers: {
            'Content-Type': 'application/json'
        },
        method: "post",
        body: formDataToJsonString(formData)
    });
}

export function Component() {
    const data = useLoaderData() as AsyncReturnType<typeof loader>;

    if (data !== null) {
        return <div>
            <Form method="post" action="/megabox">
                <label htmlFor="jsessionid">JSESSIONID</label>
                <input type="text" name="jsessionid" defaultValue={data.jsessionid} />
                <label htmlFor="session">SESSION</label>
                <input type="text" name="session" defaultValue={data.session} />
                <button type="submit">Update</button>
            </Form>
        </div>;
    } else {
        return <Navigate to="/" />;
    }
}
