# Calendar hub

Sync reservations of reservation services to google calendar.

## Setup

Working directory of docker image is root(`/`).

- Google API
    - Required API\
      Google Calendar API
    - Google API OAuth Client
        - Web application client
        - Redirection URI\
          `${URL_PREFIX}/google/callback` should be set
        - information JSON\
          calendar-hub loads this at startup time from `google.json` on working directory.
- `URL_PREFIX` environment variable\
  for generate proper external URL. ex) https://calendar-hub.example.com
- `allowed-emails` file\
  login allowed google account email per each line
