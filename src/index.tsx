import React, { useCallback } from "react";
import { createRoot } from "react-dom/client";
import {
  createRoutesFromElements,
  Outlet,
  Route,
  RouterProvider,
  useRouteLoaderData,
} from "react-router";
import {
  createBrowserRouter,
  Form,
  NavLink,
  useFormAction,
} from "react-router-dom";
import NaverReservation, {
  loadData as naverReservationLoadData,
  updateAction as naverReservationUpdateAction,
} from "./naver_reservation";
import Kobus, {
  loadData as kobusLoadData,
  updateAction as kobusUpdateAction,
} from "./kobus";
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
        </ul>
        <ul>
          <li>
            <NavLink to="/kobus">kobus</NavLink>
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
        <Route
          path="naver"
          element={<NaverReservation />}
          loader={naverReservationLoadData}
        >
          <Route path="user" action={naverReservationUpdateAction} />
        </Route>
        <Route path="kobus" element={<Kobus />} loader={kobusLoadData}>
          <Route path="user" action={kobusUpdateAction} />
        </Route>
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
