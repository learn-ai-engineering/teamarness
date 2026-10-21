-- Optional author-clock time belongs to the signed comment.add record; old rows have no time.
ALTER TABLE comments ADD COLUMN created_at TEXT;
