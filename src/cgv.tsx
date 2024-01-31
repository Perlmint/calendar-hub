import React from "react";
import { ActionFunctionArgs, Form, Navigate, useLoaderData } from "react-router-dom";
import { AsyncReturnType, formDataToJsonString } from './utils';

export async function loader() {
    const resp = await fetch("/cgv/user", {
        credentials: "same-origin",
    });

    if (resp.ok) {
        const parsed = await resp.json();
        return {
            webauth: parsed.webauth as string,
            aspxauth: parsed.aspxauth as string,
        }
    } else {
        return null;
    }
}

export async function action({ request }: ActionFunctionArgs) {
    const formData = await request.formData();
    return await fetch("/cgv/user", {
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
            <Form method="post" action="/cgv">
                <label htmlFor="webauth">WEBAUTH</label>
                <input type="text" name="webauth" defaultValue={data.webauth} />
                <label htmlFor="aspxauth">.ASPXAUTH</label>
                <input type="text" name="aspxauth" defaultValue={data.aspxauth} />
                <button type="submit">Update</button>
            </Form>
        </div>;
    } else {
        return <Navigate to="/" />;
    }
}
