CREATE TABLE `nodes` (
  `id` bigint unsigned NOT NULL AUTO_INCREMENT,
  `addr` varchar(255) COLLATE utf8mb4_general_ci NOT NULL DEFAULT '0.0.0.0:0',
  `authorization` varchar(255) COLLATE utf8mb4_general_ci DEFAULT NULL,
  `admin_authorization` varchar(255) COLLATE utf8mb4_general_ci DEFAULT NULL,
  `pub_max` bigint unsigned NOT NULL DEFAULT '0',
  `sub_max` bigint unsigned NOT NULL DEFAULT '0',
  `reforward_maximum_idle_time` bigint unsigned NOT NULL DEFAULT '0',
  `reforward_cascade` tinyint(1) NOT NULL DEFAULT '0',
  `stream` bigint unsigned NOT NULL DEFAULT '0',
  `publish` bigint unsigned NOT NULL DEFAULT '0',
  `subscribe` bigint unsigned NOT NULL DEFAULT '0',
  `reforward` bigint unsigned NOT NULL DEFAULT '0',
  `created_at` timestamp NULL DEFAULT CURRENT_TIMESTAMP,
  `updated_at` datetime DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP,
  PRIMARY KEY (`id`),
  UNIQUE KEY `uk_addr` (`addr`),
  KEY `idx_update_time` (`updated_at`),
  KEY `idx_reforward` (`reforward`)
);

CREATE TABLE `streams` (
  `id` bigint unsigned NOT NULL AUTO_INCREMENT,
  `stream` varchar(255) COLLATE utf8mb4_general_ci NOT NULL DEFAULT '',
  `addr` varchar(255) COLLATE utf8mb4_general_ci NOT NULL DEFAULT '0.0.0.0:0',
  `publish` bigint unsigned NOT NULL DEFAULT '0',
  `subscribe` bigint unsigned NOT NULL DEFAULT '0',
  `reforward` bigint unsigned NOT NULL DEFAULT '0',
  `created_at` datetime DEFAULT CURRENT_TIMESTAMP,
  `updated_at` datetime DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP,
  PRIMARY KEY (`id`),
  UNIQUE KEY `uk_stream_addr` (`stream`,`addr`),
  KEY `idx_addr` (`addr`)
);
