-- Add migration script here
CREATE TABLE IF NOT EXISTS user (
    `user_id` INTEGER PRIMARY KEY AUTOINCREMENT NOT NULL,
    `dummy` bool
);

CREATE TABLE IF NOT EXISTS google_user (
    `user_id` int primary key not null,
    `subject` text not null,
    `access_token` text not null,
    `calendar_id` text not null,
    `last_synced` datetime not null
);

CREATE TABLE IF NOT EXISTS naver_user (
    `user_id` int primary key not null,
    `aut` text not null,
    `ses` text not null
);

CREATE TABLE IF NOT EXISTS reservation (
    `id` text not null,
    `user_id` int not null,
    `title` text not null,
    `detail` text not null,
    `date_begin` date not null,
    `time_begin` time,
    `date_end` date,
    `time_end` time,
    `invalid` bool not null ,
    `updated_at` datetime not null,
    PRIMARY KEY (`id`, `user_id`)
);

CREATE TABLE IF NOT EXISTS google_event (
    `event_id` text not null,
    `user_id` int not null,
    `reservation_id` text not null,
    PRIMARY KEY (`event_id`, `user_id`),
    UNIQUE (`user_id`, `reservation_id`)
);