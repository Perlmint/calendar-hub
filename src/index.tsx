import React from "react";
import { createRoot } from "react-dom/client";
import { createRoutesFromElements, Outlet, Route, RouterProvider, useRouteLoaderData } from "react-router";
import { createBrowserRouter, NavLink } from "react-router-dom";
import NaverReservation, { loadData as naverReservationLoadData, updateAction as naverReservationUpdateAction } from './naver_reservation';
import '@picocss/pico/css/pico.classless.min.css'
import { AsyncReturnType } from "./utils";

function Layout() {
    return <>
        <nav>
            <ul>
                <li><NavLink to="/"><strong>Calendar Hub</strong></NavLink></li>
            </ul>
            <ul>
                <li><NavLink to="/naver">Naver</NavLink></li>
            </ul>
        </nav>
        <div>
            <Outlet />
        </div>
    </>;
}

function Index() {
    let logged_in = useRouteLoaderData("root") as AsyncReturnType<typeof getUser>;
    return logged_in ? <a href="/google/logout"><button>logout</button></a> : <a href="/google/login"><button>login</button></a>;
}

async function getUser() {
    const resp = await fetch("/user", {
        credentials: "same-origin",
    });

    if (resp.ok) {
        const parsed = await resp.json();
        return parsed as boolean;
    } else {
        return null;
    }
}

const router = createBrowserRouter(
    createRoutesFromElements(
        <>
            <Route path="/" id="root" loader={getUser} element={<Layout />}>
                <Route path="" element={<Index />} />
                <Route
                    path="naver"
                    element={<NaverReservation />}
                    loader={naverReservationLoadData}
                >
                    <Route path="user" action={naverReservationUpdateAction} />
                </Route>
            </Route>
        </>
    )
);

const root = createRoot(document.getElementsByTagName('main')[0])
root.render(<React.StrictMode>
    <RouterProvider router={router} />
</React.StrictMode>);
