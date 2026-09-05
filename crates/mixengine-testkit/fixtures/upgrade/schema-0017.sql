-- What `schema-0017.db` was seeded with — roadmap task T89.
--
-- Today's head, which is the schema **v0.1.0 will ship**. It proves nothing today: it is `Current`,
-- no migration runs against it, and every assertion about it is trivially true. It is exactly the
-- fixture the first real upgrade this project ever performs will need, and now is the only moment
-- at which it can be captured honestly.
--
-- Nothing in it is real; see the note at the top of `schema-0001.sql`. Unlike `schema-0015.sql`
-- this one does carry a running service and a shared site — nothing starts a daemon on it, so the
-- two shapes that seed refuses are safe here and worth having.

INSERT INTO runtime_installs
    (id, kind, version, channel, install_path, installed_at, size_bytes, source_url, sha256,
     is_default, provides_json, extension_dir, extensions_json, extension_choices_json)
VALUES
    (1, 'php',  '8.3.12', 'stable', '/home/dev/MixEngine/runtimes/php/8.3.12',
     '2026-01-04T09:00:00Z', 82000000, 'https://example.invalid/php-8.3.12.tar.zst', 'aa01', 1,
     '{"php":"bin/php","php-fpm":"sbin/php-fpm"}',
     '/home/dev/MixEngine/runtimes/php/8.3.12/lib/php/extensions',
     '{"redis":"6.0.2"}', '{"opcache":"on"}'),
    (2, 'php',  '8.2.20', 'stable', '/home/dev/MixEngine/runtimes/php/8.2.20',
     '2026-01-04T09:05:00Z', 80000000, 'https://example.invalid/php-8.2.20.tar.zst', 'aa02', 0,
     '{"php":"bin/php"}', '', '{}', '{}'),
    (3, 'node', '22.8.0', 'stable', '/home/dev/MixEngine/runtimes/node/22.8.0',
     '2026-01-04T09:10:00Z', 51000000, 'https://example.invalid/node-22.8.0.tar.zst', 'aa03', 1,
     '{"node":"bin/node","npm":"bin/npm"}', '', '{}', '{}');

INSERT INTO packages
    (id, name, version, install_path, installed_at, source_url, sha256, size_bytes, provides_json)
VALUES
    (1, 'mariadb', '11.4.2', '/home/dev/MixEngine/packages/mariadb/11.4.2',
     '2026-01-04T09:15:00Z', 'https://example.invalid/mariadb-11.4.2.tar.zst', 'bb01', 340000000,
     '{"mariadbd":"bin/mariadbd"}'),
    (2, 'caddy',   '2.11.4', '/home/dev/MixEngine/packages/caddy/2.11.4',
     '2026-01-04T09:16:00Z', 'https://example.invalid/caddy-2.11.4.tar.zst',   'bb02',  48000000,
     '{"caddy":"caddy"}');

-- Before the services that name it: 0016's `services` has a foreign key into this table.
INSERT INTO extensions
    (id, name, version, kind, manifest_json, install_dir, data_dir, source, signed, installed_at)
VALUES
    ('mailpit', 'Mailpit', '1.20.0', 'service',
     '{"schema":1,"extension":{"id":"mailpit"}}',
     '/home/dev/MixEngine/extensions/mailpit',
     '/home/dev/MixEngine/data/extensions/mailpit', 'registry', 1, '2026-01-04T09:50:00Z'),
    ('mixdb',   'MixDB',   '0.4.1',  'desktop-app',
     '{"schema":1,"extension":{"id":"mixdb"}}',
     '/home/dev/MixEngine/extensions/mixdb',
     '/home/dev/MixEngine/data/extensions/mixdb',   'registry', 1, '2026-01-04T09:51:00Z');

INSERT INTO extension_ports (extension_id, name, port)
VALUES ('mailpit', 'http', 8025), ('mailpit', 'smtp', 1025);

-- The third parent a service can have, which 0016 is the migration that added.
INSERT INTO services
    (id, package_id, runtime_install_id, extension_id, instance_name, state, autostart, port,
     activation_port, bind_addr, data_dir, config_overrides_json, limits_json, idle_minutes,
     idle_stopped, last_started_at, last_exit_code, pid, pid_start_time)
VALUES
    ('mariadb@main', 1, NULL, NULL, 'main', 'stopped', 1, 3306, 13306, '127.0.0.1',
     '/home/dev/MixEngine/data/mariadb/main', '{"innodb_buffer_pool_size":"256M"}', '{}', 30, 1,
     1767517200000, 0, NULL, NULL),
    ('caddy@main',   2, NULL, NULL, 'main', 'running', 1,   80, NULL, '127.0.0.1',
     NULL, '{}', '{}', NULL, 0, 1767517260000, NULL, 4242, 99887766),
    ('php-fpm@8.3',  NULL, 1, NULL, '8.3',  'stopped', 0, 9000, 19000, '127.0.0.1',
     NULL, '{}', '{"memory_mb":512}', 10, 0, NULL, NULL, NULL, NULL),
    ('mailpit@main', NULL, NULL, 'mailpit', 'main', 'running', 1, 8025, NULL, '127.0.0.1',
     '/home/dev/MixEngine/data/extensions/mailpit', '{}', '{}', NULL, 0,
     1767517280000, NULL, 4343, 99887799);

INSERT INTO blueprints
    (id, name, description, manifest_toml, created_at, source, trusted, signature)
VALUES
    ('laravel',       'Laravel',       'A Laravel project', 'schema = 1',
     '2026-01-04T09:20:00Z', 'builtin',  1, NULL),
    ('my-shop',       'My shop',       '',                  'schema = 1',
     '2026-01-04T09:21:00Z', 'captured', 1, NULL),
    ('from-a-friend', 'From a friend', '',                  'schema = 1',
     '2026-01-04T09:22:00Z', 'imported', 0, NULL);

INSERT INTO projects
    (id, name, root_path, runtime_pins_json, created_at, blueprint_id, keep_warm)
VALUES
    (1, 'blog', '/home/dev/blog', '{"php":"8.3.12"}', '2026-01-04T09:30:00Z', 'laravel', 1),
    (2, 'shop', '/home/dev/shop', '{}',               '2026-01-04T09:31:00Z', NULL,      0);

-- A project's site and an extension's — the two halves of 0017's exclusive-or — and a shared one,
-- which is three of the four combinations 0013's trigger constrains.
INSERT INTO sites
    (id, project_id, extension_id, doc_root, kind, php_service_id, https_enabled, http_port,
     https_port, config_json, state, shared_interface, shared_address, shared_since, shared_until)
VALUES
    (1, 1, NULL, '/home/dev/blog/public', 'php-fpm', 'php-fpm@8.3', 1, 80, 443, '{}', 'enabled',
     'en0', '192.168.1.24', 1767517300000, 1767520900000),
    (2, 2, NULL, '/home/dev/shop/public', 'static',  NULL,          0, 80, 443, '{}', 'disabled',
     NULL, NULL, NULL, NULL),
    (3, NULL, 'mailpit', '/home/dev/MixEngine/extensions/mailpit/public', 'reverse-proxy', NULL,
     1, 80, 443, '{"upstream":"127.0.0.1:8025"}', 'enabled', NULL, NULL, NULL, NULL);

INSERT INTO site_domains (id, site_id, domain, is_primary)
VALUES (1, 1, 'blog.test', 1), (2, 1, 'www.blog.test', 0), (3, 2, 'shop.test', 1),
       (4, 3, 'mailpit.test', 1);

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

INSERT INTO pending_privileged_ops (id, op, dedupe_key, requested_at)
VALUES (1, 'hosts.apply', 'hosts:blog.test', 1767517400000);

INSERT INTO metrics_minutes (subject, minute, cpu_avg, cpu_peak, rss_avg, rss_peak, samples)
VALUES ('caddy@main',   29458620, 0.4, 1.2, 41943040, 46137344, 12),
       ('mariadb@main', 29458620, 1.1, 3.7, 268435456, 289406976, 12);
