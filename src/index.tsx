import React, { useCallback } from "react";
import { createRoot } from "react-dom/client";
import {
  createRoutesFromElements,
  Outlet,
  Route,
  RouterProvider,
  useRouteLoaderData,
} from "react-router";
import { createBrowserRouter, Form, NavLink } from "react-router-dom";
import "@picocss/pico/css/pico.classless.min.css";
import { AsyncReturnType } from "./utils";

function Layout() {
  return (
    <>
      <nav>
        <ul>
          <li>
            <NavLink to="/">
              <strong>Calendar Hub</strong>
            </NavLink>
          </li>
        </ul>
        <ul>
          <li>
            <NavLink to="/naver">Naver</NavLink>
          </li>
          <li>
            <NavLink to="/kobus">kobus</NavLink>
          </li>
          <li>
            <NavLink to="/catch-table">catch table</NavLink>
          </li>
          <li>
            <NavLink to="/cgv">cgv</NavLink>
          </li>
          <li>
            <NavLink to="/megabox">MEGABOX</NavLink>
          </li>
          <li>
            <NavLink to="/bustago">Bustago</NavLink>
          </li>
        </ul>
      </nav>
      <div>
        <Outlet />
      </div>
    </>
  );
}

function Index() {
  const logged_in = useRouteLoaderData("user") as AsyncReturnType<
    typeof getUser
  >;
  if (logged_in !== null) {
    return (
      <>
        <Form method="post" action="/">
          <button className="primary" type="submit">
            sync (last: {logged_in.last_synced.toLocaleString()})
          </button>
        </Form>
        <a href="/logout">
          <button>logout</button>
        </a>
      </>
    );
  } else {
    return (
      <a href="/login">
        <button>login</button>
      </a>
    );
  }
}

async function getUser() {
  const resp = await fetch("/user", {
    credentials: "same-origin",
  });

  if (resp.ok) {
    const parsed = await resp.json();
    switch (parsed.type as "None" | "User") {
      case "None":
        return null;
      case "User":
        return {
          last_synced: new Date(parsed.last_synced),
        };
      default:
        return null;
    }
  } else {
    return null;
  }
}

const router = createBrowserRouter(
  createRoutesFromElements(
    <>
      <Route
        path="/"
        id="user"
        loader={getUser}
        action={async () => {
          const resp = await fetch("/sync", {
            method: "POST",
            credentials: "same-origin",
          });

          return await resp.json();
        }}
        element={<Layout />}
      >
        <Route path="" element={<Index />} />
        <Route path="naver" lazy={() => import("./naver_reservation")} />
        <Route path="kobus" lazy={() => import("./kobus")} />
        <Route path="catch-table" lazy={() => import("./catch_table")} />
        <Route path="cgv" lazy={() => import("./cgv")} />
        <Route path="megabox" lazy={() => import("./megabox")} />
        <Route path="bustago" lazy={() => import("./bustago")} />
      </Route>
    </>
  )
);

const root = createRoot(document.getElementsByTagName("main")[0]);
root.render(
  <React.StrictMode>
    <RouterProvider router={router} />
  </React.StrictMode>
);
