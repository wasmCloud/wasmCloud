CREATE TABLE IF NOT EXISTS users (
    id    SERIAL PRIMARY KEY,
    name  TEXT NOT NULL,
    email TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS todos (
    id    SERIAL PRIMARY KEY,
    title TEXT NOT NULL,
    done  BOOLEAN NOT NULL DEFAULT FALSE
);

INSERT INTO users (name, email) VALUES
    ('Ada Lovelace', 'ada@example.com'),
    ('Alan Turing', 'alan@example.com'),
    ('Grace Hopper', 'grace@example.com');

INSERT INTO todos (title, done) VALUES
    ('Write a wasi:http service', TRUE),
    ('Pool connections to Postgres', TRUE),
    ('Ship the template', FALSE);
