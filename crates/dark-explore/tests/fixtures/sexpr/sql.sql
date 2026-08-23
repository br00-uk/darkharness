CREATE TABLE users (id INT, name TEXT);

CREATE VIEW active_users AS
SELECT id, name FROM users WHERE id > 0;
