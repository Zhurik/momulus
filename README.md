# llm-bot

Оркестратор LLM-скиллов для pull request'ов. Следит за комментариями в PR, по
команде `/llm <skill> [args]` запускает детерминированный пайплайн с **одним**
LLM-шагом (coding-агент [pi](https://www.npmjs.com/package/@earendil-works/pi-coding-agent)
в контейнере) и публикует результат обратно в PR.

```
Trigger ──► Job ──► Queue(SQLite) ──► Pipeline ──► Publisher
                                         │
         workspace → skill resolve → Runner(pi в docker) → validate
```

Без веб-UI, без брокеров, без k8s: SQLite и один процесс. Никаких GitHub Actions
и self-hosted runner'ов — сервис сам опрашивает API.

## Что он умеет

- **review-скиллы** — модель ищет замечания, бот публикует одно ревью
  с inline-комментариями и блоками ```suggestion```. Замечания на строках вне
  diff и про файлы вне области скилла не выбрасываются, а уходят в резюме.
- **patch-скиллы** — модель правит файлы в рабочей копии, бот коммитит от своего
  имени, пушит ветку и открывает PR с base = head-ветка исходного PR.
- Статусы видно по реакциям на комментарий-команду: 👀 взято в работу,
  ✅ готово, ❌ провал (плюс комментарий с краткой причиной и id джобы).

В поставке три скилла: `proofread` (вычитка русскоязычных MDX-статей),
`review` (ревью кода), `translate` (перевод статей, режим patch).

## Требования

- Linux x86_64, 4–8 ГБ RAM, публичный IP не нужен.
- Docker и docker-compose.
- GitHub App (как создать — ниже) и ключ LLM-провайдера.

## Быстрый старт

```bash
# 1. Собрать образ с pi и образ сервиса
just build-images

# 2. Проверить скиллы
just skills-validate

# 3. Прогнать скилл локально, без GitHub — полезно для отладки промптов
export LLM_API_KEY=...            # ключ провайдера
export LLM_BASE_URL=https://foundation-models.api.cloud.ru/v1
cargo run -p llm-bot -- run \
    --repo-path ~/Projects/blog \
    --skill proofread \
    --files content/posts/dns/index.ru.md \
    --out ./out
```

Результат: `./out/findings.json` (замечания) и `./out/pi.log` (полный вывод
агента). Для patch-скиллов правки остаются в рабочей копии — смотри `git diff`.

## Создание GitHub App

1. **Settings → Developer settings → GitHub Apps → New GitHub App.**
2. Homepage URL — что угодно, webhook можно выключить: бот работает опросом.
3. Права репозитория (Repository permissions):

   | Право | Уровень | Зачем |
   |---|---|---|
   | Contents | Read and write | клонировать репозиторий, пушить ветку патча |
   | Pull requests | Read and write | читать PR, публиковать ревью, открывать PR |
   | Issues | Read and write | комментарии в PR и реакции на них |
   | Metadata | Read-only | обязательное право |

4. **Generate a private key**, скачать `.pem`.
5. **Install App** в нужные репозитории.
6. Запомнить **App ID** (страница приложения) — он пойдёт в `GITHUB_APP_ID`.

Установку по репозиторию сервис находит сам (`GET /repos/{owner}/{repo}/installation`)
и кэширует installation-токены, так что несколько установок работают из одного
процесса.

## Конфигурация

`config.toml` — всё, кроме секретов (пример: `config.example.toml`):

```toml
poll_interval = "45s"          # как часто опрашиваем GitHub
concurrency = 1                # сколько джоб одновременно
allowed_users = ["zhurik"]     # кому разрешены команды
repos = []                     # белый список репозиториев; пусто — все установки
data_dir = "/srv/llm-bot/data" # БД, кэш репозиториев, worktree, логи
skills_dir = "/srv/llm-bot/skills"
shutdown_timeout = "5m"

[llm]
default_provider = "cloudru"
default_model = "zai-org/GLM-5.1"
api = "openai-completions"     # только для провайдеров, которых нет в pi

[docker]
runner_image = "llm-bot-runner:latest"
cpu = 2.0
memory_mb = 2048
# user = "1000:1000"           # если uid сервиса не совпадает с uid в образе

[limits]
max_diff_bytes = 400000        # слишком большой PR → отказ без вызова модели
max_file_bytes = 200000
max_changed_files = 100
```

Секреты только через окружение:

| Переменная | Что это |
|---|---|
| `GITHUB_APP_ID` | id GitHub App |
| `GITHUB_APP_PRIVATE_KEY_PATH` | путь к `.pem` ключу App |
| `LLM_API_KEY` | ключ LLM-провайдера |
| `LLM_BASE_URL` | база API провайдера (для Cloud.ru — `https://foundation-models.api.cloud.ru/v1`) |

### Провайдер, которого pi не знает

pi знает встроенных провайдеров (`anthropic`, `openai`, `google`, …) — для них
достаточно `default_provider`/`default_model`, а `api` из конфига нужно убрать.

Для OpenAI-совместимого шлюза (как Cloud.ru Foundation Models) задаётся
`api = "openai-completions"`. Тогда llm-bot генерирует в контейнере `models.json`
с `baseUrl`, `api` и списком моделей, а ключ в файл **не** попадает: там стоит
`$CLOUDRU_API_KEY`, и значение pi берёт из переменной окружения контейнера.
Имя переменной выводится из имени провайдера (`cloudru` → `CLOUDRU_API_KEY`) или
задаётся явно через `llm.api_key_env`.

## Деплой через docker-compose

```bash
export LLM_BOT_ROOT=/srv/llm-bot
mkdir -p $LLM_BOT_ROOT/{data,secrets}
cp config.example.toml $LLM_BOT_ROOT/config.toml   # поправить пути и allowed_users
cp -r skills $LLM_BOT_ROOT/skills
cp ~/github-app.private-key.pem $LLM_BOT_ROOT/secrets/github-app.pem

cat > .env <<'ENV'
LLM_BOT_ROOT=/srv/llm-bot
GITHUB_APP_ID=123456
LLM_API_KEY=...
LLM_BASE_URL=https://foundation-models.api.cloud.ru/v1
ENV

docker compose --profile build build   # образ с pi
docker compose up -d
docker compose logs -f llm-bot
```

### Почему пути на хосте и в контейнере совпадают

Сервис не запускает pi сам — он просит docker-демон создать контейнер и
смонтировать в него рабочую копию. Демон живёт на хосте и понимает **только
хостовые пути**. Поэтому `data_dir` и `skills_dir` заданы абсолютными путями и
смонтированы «сами в себя» (`/srv/llm-bot/data:/srv/llm-bot/data`). Если
смонтировать их куда-то ещё, раннер получит путь, которого на хосте нет.

### Риск доступа к docker.sock

`/var/run/docker.sock` в контейнере — это фактически root на хосте: кто может
создавать контейнеры, может смонтировать `/` и выйти за пределы контейнера.
Сервис написан так, что в контейнер с моделью не попадает ни GitHub-токен, ни
что-либо кроме ключа провайдера, но сам доступ к сокету остаётся мощным.

Вариант поспокойнее — [docker-socket-proxy](https://github.com/Tecnativa/docker-socket-proxy),
который пропускает только нужные эндпоинты:

```yaml
services:
  docker-proxy:
    image: tecnativa/docker-socket-proxy:latest
    environment:
      CONTAINERS: 1      # create/start/wait/logs/kill/remove
      IMAGES: 1          # inspect образа раннера
      POST: 1            # без этого нельзя создавать контейнеры
      # всё остальное (VOLUMES, NETWORKS, EXEC, SWARM, INFO...) выключено
    volumes:
      - /var/run/docker.sock:/var/run/docker.sock:ro
    restart: unless-stopped

  llm-bot:
    environment:
      DOCKER_HOST: tcp://docker-proxy:2375
    # и убрать монтирование /var/run/docker.sock
```

Полной изоляции это не даёт (право создавать контейнеры с bind-mount остаётся),
но убирает всё лишнее и оставляет аудируемый шлюз.

## Как это работает внутри

1. **Trigger** раз в `poll_interval` читает `GET /repos/{o}/{r}/issues/comments?since=…`
   и `…/pulls/comments?since=…` для каждого репозитория установки. Курсор на
   репозиторий и поток лежит в SQLite, дедупликация — по id комментария.
   Берутся только комментарии к PR, начинающиеся с `/llm`, от `allowed_users`.
   Остальное игнорируется молча; на непонятную команду бот отвечает списком
   скиллов. Rate limit: читаются `X-RateLimit-*` и `Retry-After`, повтор
   с экспоненциальной задержкой и джиттером.
2. **Queue** — таблица `jobs` в SQLite: статусы `queued/running/done/failed`,
   попытки, ошибка, путь к логу. При старте всё, что осталось в `running`,
   возвращается в очередь. Ретраятся только транзиентные ошибки (сеть, 5xx,
   429, таймаут), максимум две повторные попытки.
3. **Workspace** — bare-кэш `data/repos/<owner>/<repo>.git`, на джобу
   `git worktree` на head-коммите PR в `data/work/<job_id>`. Каталог удаляется
   гарантированно, в том числе при панике. Токен передаётся git через
   credential helper из переменной окружения: в remote URL на диске и в логах
   его нет.
4. **Runner** — контейнер с pi на джобу: `/work` (ro для review, rw для patch),
   `/skills` (ro), `/out` (rw), лимиты CPU/RAM, `cap_drop: ALL`,
   `no-new-privileges`, таймаут из контракта скилла (по таймауту контейнер
   убивается). Полный stdout/stderr агента сохраняется в `data/logs/<job_id>.log`.
5. **Validate** — строгая десериализация `/out/findings.json` (неизвестные поля
   запрещены). Невалидный JSON → одна повторная попытка с текстом ошибки
   в промпте, затем провал. Замечания вне hunks и про чужие файлы переносятся
   в резюме, лишние обрезаются по `max_comments` со счётчиком.
6. **Publisher** — одно `POST /repos/{o}/{r}/pulls/{n}/reviews` с `event=COMMENT`
   для review; для patch — коммит, ветка (при коллизии добавляется short SHA),
   push и `POST /pulls`. PR из форка в режиме patch не поддерживается — бот
   пишет об этом комментарием.

## CLI

```bash
llm-bot serve [--dry-run]            # основной режим; --dry-run ничего не публикует
llm-bot run --repo-path <dir> --skill <name> [--files a.mdx,b.mdx] [--arg k=v]
llm-bot skills list                  # какие скиллы есть
llm-bot skills validate              # проверить контракты
```

Общие флаги: `--config`, `--skills-dir`, `--log-format text|json`, `--log-level`.

`SIGHUP` перечитывает каталог скиллов на живом сервисе (если новый контракт
битый, остаётся прежний реестр). `SIGTERM`/`SIGINT` — остановка: новые джобы не
берутся, текущие дожидаются в пределах `shutdown_timeout`.

## Как добавить скилл

Каталог `skills/<name>/` с двумя файлами.

`SKILL.md` — инструкции для модели в формате Agent Skills:

```markdown
---
name: my-skill
description: Что делает и когда применять — по описанию модель понимает, брать ли скилл
---

# My skill

Инструкции: что искать, чего не делать, как оформить результат.
```

`skill.toml` — контракт для оркестратора:

```toml
mode = "review"                              # review | patch
tools = ["read", "grep", "find", "ls", "write"]  # передаётся pi как --tools
files = ["**/*.md", "**/*.mdx"]              # фильтр изменённых файлов PR; пусто = все
args = []                                    # напр. ["lang"] для translate
timeout = "10m"
model = ""                                   # пусто = модель из конфига
max_comments = 30                            # только review
# branch = "llm/{skill}-{pr}"                # только patch; доступны и имена args

[vars]                                       # произвольные параметры, уходят в промпт
# naming = "{dir}/{stem}.{lang}{ext}"
```

Тонкости, на которые стоит обратить внимание:

- `write` в `tools` обязателен: без него агент не сможет записать
  `/out/findings.json`. Для review это безопасно — `/work` смонтирован
  read-only, так что писать он может только в `/out`.
- Если под `files` не попал ни один изменённый файл PR, бот пишет
  «нечего делать» и **не** вызывает модель.
- `llm-bot skills validate` ловит неизвестные поля, пустые `tools` у patch,
  битые globs, `max_comments` в patch-режиме и неизвестные placeholder'ы в `branch`.

## Как добавить платформу (GitLab, Forgejo)

Ядро (`core`, `skills`, `workspace`, `queue`, `pipeline`) не знает ни про
octocrab, ни про bollard. Чтобы добавить платформу, нужен новый крейт с
реализациями трейтов из `llm-bot-core`:

| Трейт | Что реализовать |
|---|---|
| `Trigger` | откуда узнаём о командах: опрос API, курсоры через `CursorStore`, разбор `/llm` через `Command::parse` |
| `Publisher` | `ack` (реакции), `post_review` (ревью с inline-комментариями), `push_and_open_pr`, `comment` |
| `GitAccess` | токен для git и refspec'и, которыми достаётся head PR (у GitHub это `refs/pull/<n>/head`) |

Всё остальное — очередь, рабочие копии, промпты, валидация, ретраи — переиспользуется
как есть. Собрать граф зависимостей нужно в `crates/llm-bot/src/commands/serve.rs`.

Готовые вспомогательные реализации для тестов: `MemoryCursorStore`,
`StaticSkillCatalog`, `LocalGitAccess`, `StdoutPublisher`, `FakeRunner`.

## Безопасность

- Команды выполняются только от `allowed_users`, опционально ограничен
  список репозиториев.
- Содержимое PR — недоверенный ввод (prompt injection). У агента **нет**
  GitHub-токена, минимальный набор tools, в review-режиме рабочая копия
  смонтирована только для чтения, коммит и push делает оркестратор, а не модель.
- Секреты не логируются: токены и ключи маскируются в выводе git и pi
  (`Redactor`, есть тесты), в сообщениях об ошибках нет стектрейсов.
- Размер входа ограничен: слишком большой diff или файл → отказ с комментарием
  до вызова модели.

## Разработка

```bash
just test               # fmt + clippy + cargo test
just test-integration   # тесты, которым нужны Docker и настоящий pi
just build-images       # llm-bot-runner и llm-bot
```

Структура workspace:

```
crates/
  core/            Job, PrRef, Finding, Patch, трейты, ошибки, конфиг, маскирование
  skills/          реестр скиллов, skill.toml, валидация контрактов
  workspace/       bare-кэш, git worktree, парсер unified diff, маппинг строк
  queue/           SQLite, миграции, жизненный цикл джобы, воркер
  pipeline/        оркестрация джобы, промпт, валидация, рендер публикаций
  runner-docker/   Runner через bollard
  github/          Trigger (polling) + Publisher на octocrab
  llm-bot/         бинарник: clap, конфиг, сборка графа зависимостей
skills/            proofread, translate, review
docker/            Dockerfile сервиса и Dockerfile.runner с pi
migrations/        миграции sqlx
```

Тесты не ходят в сеть: GitHub подменяется `wiremock`, LLM — `FakeRunner`,
remote — локальный bare-репозиторий в `tempfile`. Тесты, которым нужен Docker и
настоящий pi, помечены `#[ignore]` и запускаются отдельно; без `LLM_API_KEY`
они сообщают об этом и не падают.
