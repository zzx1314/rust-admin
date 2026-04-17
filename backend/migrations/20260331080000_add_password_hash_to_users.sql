-- Migration: add password_hash column to users (nullable for existing rows)
ALTER TABLE users
ADD COLUMN password_hash TEXT;
