-- Add migration script here
ALTER TABLE `reservation` ADD COLUMN `location` TEXT;
ALTER TABLE `reservation` ADD COLUMN `url` TEXT;