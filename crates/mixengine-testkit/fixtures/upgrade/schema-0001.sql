-- What `schema-0001.db` was seeded with — roadmap task T89.
--
-- This file is not read by any test. It is here so that the blob beside it is reviewable, and so
-- that the next capture starts from something rather than from nothing.
--
-- Nothing in it is real: every path is a `/home/dev` literal that exists nowhere, and the `ca` and
-- `certificates` rows carry *paths to* key files rather than keys, which is what makes them safe to
-- commit at all.

-- Runtimes: one default per kind, and a second PHP that is not the default.
INSERT INTO runtime_installs
    (id, kind, version, channel, install_path, installed_at, size_bytes, source_url, sha256,
     is_default)
VALUES
    (1, 'php',  '8.3.12', 'stable', '/home/dev/MixEngine/runtimes/php/8.3.12',
     '2026-01-04T09:00:00Z', 82000000, 'https://example.invalid/php-8.3.12.tar.zst',  'aa01', 1),
    (2, 'php',  '8.2.20', 'stable', '/home/dev/MixEngine/runtimes/php/8.2.20',
     '2026-01-04T09:05:00Z', 80000000, 'https://example.invalid/php-8.2.20.tar.zst',  'aa02', 0),
    (3, 'node', '22.8.0', 'stable', '/home/dev/MixEngine/runtimes/node/22.8.0',
     '2026-01-04T09:10:00Z', 51000000, 'https://example.invalid/node-22.8.0.tar.zst', 'aa03', 1);

INSERT INTO packages (id, name, version, install_path, installed_at, source_url, sha256)
VALUES
    (1, 'mariadb', '11.4.2', '/home/dev/MixEngine/packages/mariadb/11.4.2',
     '2026-01-04T09:15:00Z', 'https://example.invalid/mariadb-11.4.2.tar.zst', 'bb01'),
    (2, 'caddy',   '2.11.4', '/home/dev/MixEngine/packages/caddy/2.11.4',
     '2026-01-04T09:16:00Z', 'https://example.invalid/caddy-2.11.4.tar.zst',   'bb02');

-- Both parents a service can have, so the rebuild at 0016 has one of each to carry across.
INSERT INTO services
    (id, package_id, runtime_install_id, instance_name, state, autostart, port, bind_addr,
     data_dir, config_overrides_json, limits_json, idle_minutes, last_started_at, last_exit_code,
     pid, pid_start_time)
VALUES
    ('mariadb@main', 1, NULL, 'main', 'stopped', 1, 3306, '127.0.0.1',
     '/home/dev/MixEngine/data/mariadb/main', '{"innodb_buffer_pool_size":"256M"}', '{}', 30,
     1767517200000, 0, NULL, NULL),
    ('caddy@main',   2, NULL, 'main', 'running', 1,   80, '127.0.0.1',
     NULL, '{}', '{}', NULL, 1767517260000, NULL, 4242, 99887766),
    ('php-fpm@8.3',  NULL, 1, '8.3',  'stopped', 0, 9000, '127.0.0.1',
     NULL, '{}', '{"memory_mb":512}', 10, NULL, NULL, NULL, NULL);

-- One of each source, which is what makes 0014's and 0015's UPDATEs land on different rows.
INSERT INTO blueprints (id, name, description, manifest_toml, created_at, source)
VALUES
    ('laravel',       'Laravel',       'A Laravel project', 'schema = 1',
     '2026-01-04T09:20:00Z', 'builtin'),
    ('my-shop',       'My shop',       '',                  'schema = 1',
     '2026-01-04T09:21:00Z', 'captured'),
    ('from-a-friend', 'From a friend', '',                  'schema = 1',
     '2026-01-04T09:22:00Z', 'imported');

INSERT INTO projects (id, name, root_path, runtime_pins_json, created_at, blueprint_id)
VALUES
    (1, 'blog', '/home/dev/blog', '{"php":"8.3.12"}', '2026-01-04T09:30:00Z', 'laravel'),
    (2, 'shop', '/home/dev/shop', '{}',               '2026-01-04T09:31:00Z', NULL);

INSERT INTO sites
    (id, project_id, doc_root, kind, php_service_id, https_enabled, http_port, https_port,
     config_json, state)
VALUES
    (1, 1, '/home/dev/blog/public', 'php-fpm', 'php-fpm@8.3', 1, 80, 443, '{}', 'enabled'),
    (2, 2, '/home/dev/shop/public', 'static',  NULL,          0, 80, 443, '{}', 'disabled');

INSERT INTO site_domains (id, site_id, domain, is_primary)
VALUES (1, 1, 'blog.test', 1), (2, 1, 'www.blog.test', 0), (3, 2, 'shop.test', 1);

INSERT INTO site_service_links (site_id, service_id) VALUES (1, 'mariadb@main');

INSERT INTO ca (id, fingerprint, cert_path, key_path, created_at, installed_in_trust_store)
VALUES (1, 'ab:cd:ef:01', '/home/dev/MixEngine/certs/ca/ca.crt',
        '/home/dev/MixEngine/certs/ca/ca.key', '2026-01-04T09:40:00Z', 1);

INSERT INTO certificates
    (id, domain, sans_json, not_before, not_after, cert_path, key_path,
     issued_by_ca_fingerprint, revoked)
VALUES
    (1, 'blog.test', '["blog.test","www.blog.test"]', '2026-01-04T09:41:00Z',
     '2027-01-04T09:41:00Z', '/home/dev/MixEngine/certs/blog.test.crt',
     '/home/dev/MixEngine/certs/blog.test.key', 'ab:cd:ef:01', 0);

INSERT INTO extensions (id, name, version, manifest_toml, install_path, state, settings_json)
VALUES ('mailpit', 'Mailpit', '1.20.0', 'schema = 1',
        '/home/dev/MixEngine/extensions/mailpit', 'enabled', '{}');

-- One finished job and one still going, which is the pair the two CHECKs on that table constrain.
INSERT INTO jobs (id, kind, state, percent, message, started_at, finished_at, result_json)
VALUES
    (1, 'runtime.install', 'succeeded', 100, 'php 8.3.12 installed',
     1767516000000, 1767516120000, '{"version":"8.3.12"}'),
    (2, 'cert.issue',      'running',    40, 'issuing blog.test',
     1767517000000, NULL, NULL);

INSERT INTO events (id, ts, kind, subject, payload_json)
VALUES
    (1, '2026-01-04T09:45:00Z', 'site.created',    'blog.test',  '{}'),
    (2, '2026-01-04T09:46:00Z', 'service.started', 'caddy@main', '{"pid":4242}');

INSERT INTO settings (key, value_json)
VALUES ('telemetry', 'false'), ('update.channel', '"stable"');
