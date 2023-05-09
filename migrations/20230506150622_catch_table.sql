-- Add migration script here
CREATE TABLE IF NOT EXISTS catch_table_user (
    `user_id` int primary key not null,
    `jsessionid` text not null
);