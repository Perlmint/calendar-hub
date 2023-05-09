-- Add migration script here
ALTER TABLE `google_user` DROP COLUMN `access_token`;
ALTER TABLE `google_user` ADD COLUMN `acl_id` TEXT;