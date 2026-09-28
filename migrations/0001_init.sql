-- Джобы: одна команда `/llm` из одного комментария.
CREATE TABLE jobs (
    id             TEXT    PRIMARY KEY,
    platform       TEXT    NOT NULL,
    repo           TEXT    NOT NULL,
    pr             INTEGER NOT NULL,
    comment_id     INTEGER NOT NULL,
    comment_kind   TEXT    NOT NULL,
    comment_author TEXT    NOT NULL,
    skill          TEXT    NOT NULL,
    args           TEXT    NOT NULL,
    pr_json        TEXT    NOT NULL,
    status         TEXT    NOT NULL,
    attempts       INTEGER NOT NULL DEFAULT 0,
    error          TEXT,
    created_at     TEXT    NOT NULL,
    started_at     TEXT,
    finished_at    TEXT,
    log_path       TEXT
);

-- Один комментарий порождает не больше одной джобы.
CREATE UNIQUE INDEX jobs_comment_unique ON jobs (platform, comment_id);
CREATE INDEX jobs_status_created ON jobs (status, created_at);

-- Курсор опроса на каждый репозиторий и поток комментариев.
CREATE TABLE repo_cursors (
    repo_key   TEXT NOT NULL,
    stream     TEXT NOT NULL,
    cursor     TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (repo_key, stream)
);

-- Все уже разобранные комментарии, включая те, что не стали джобами
-- (невалидная команда, чужой автор) — чтобы не отвечать на них дважды.
CREATE TABLE seen_comments (
    platform   TEXT    NOT NULL,
    comment_id INTEGER NOT NULL,
    seen_at    TEXT    NOT NULL,
    PRIMARY KEY (platform, comment_id)
);
