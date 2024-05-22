-- Add migration script here
CREATE TABLE IF NOT EXISTS megabox_user (
    `user_id` int primary key not null,
    `jsessionid` text not null,
    `session` text not null
);