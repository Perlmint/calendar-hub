-- Add migration script here
CREATE TABLE IF NOT EXISTS bustago_user (
    `user_id` int primary key not null,
    `jsessionid` text not null,
    `user_number` text not null
);