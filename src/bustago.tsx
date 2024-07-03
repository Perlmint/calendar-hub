import React from "react";
import { ActionFunctionArgs, Form, Navigate, Params, useLoaderData } from "react-router-dom";
import { AsyncReturnType, formDataToJsonString } from './utils';

export async function loader() {
    const resp = await fetch("/bustago/user", {
        credentials: "same-origin",
    });

    if (resp.ok) {
        const parsed = await resp.json();
        return {
            jsessionid: parsed.jsessionid as string,
            user_number: parsed.user_number as string,
        }
    } else {
        return null;
    }
}

export async function action({ request }: ActionFunctionArgs) {
    const formData = await request.formData();
    return await fetch("/bustago/user", {
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
            <Form method="post" action="/bustago">
                <label htmlFor="jsessionid">jsessionid</label>
                <input type="text" name="jsessionid" defaultValue={data.jsessionid} />
                <label htmlFor="user_number">user_number</label>
                <input type="text" name="user_number" defaultValue={data.user_number} />
                <button type="submit">Update</button>
            </Form>
        </div>;
    } else {
        return <Navigate to="/" />;
    }
}
