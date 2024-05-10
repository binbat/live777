create table nodes (
    id                          bigint unsigned auto_increment primary key,
    addr                        varchar(255)    default '0.0.0.0:0'       not null,
    authorization               varchar(255)                              null,
    admin_authorization         varchar(255)                              null,
    pub_max                     bigint unsigned default '0'               not null,
    sub_max                     bigint unsigned default '0'               not null,
    reforward_maximum_idle_time bigint unsigned default '0'               not null,
    reforward_cascade           tinyint(1)      default 0                 not null,
    stream                      bigint unsigned default '0'               not null,
    publish                     bigint unsigned default '0'               not null,
    subscribe                   bigint unsigned default '0'               not null,
    reforward                   bigint unsigned default '0'               not null,
    created_at                  timestamp       default CURRENT_TIMESTAMP null,
    updated_at                  datetime        default CURRENT_TIMESTAMP null on update CURRENT_TIMESTAMP,
    constraint uk_addr
        unique (addr)
);

create table streams (
    id         bigint unsigned auto_increment primary key,
    stream     varchar(255)    default ''                not null,
    addr       varchar(255)    default '0.0.0.0:0'       not null,
    publish    bigint unsigned default '0'               not null,
    subscribe  bigint unsigned default '0'               not null,
    reforward  bigint unsigned default '0'               not null,
    created_at datetime        default CURRENT_TIMESTAMP null,
    updated_at datetime        default CURRENT_TIMESTAMP null on update CURRENT_TIMESTAMP,
    constraint uk_stream_addr
        unique (stream, addr)
);
