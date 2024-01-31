-- Add migration script here
CREATE TABLE IF NOT EXISTS cgv_user (
    `user_id` int primary key not null,
    `webauth` text not null,
    `aspxauth` text not null
);