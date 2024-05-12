use crate::{
    error::AppError,
    model::{Node, Stream},
    result::Result,
};
use chrono::Utc;
use sqlx::MySqlPool;

impl Node {
    pub async fn db_find_not_deactivate_nodes(pool: &MySqlPool) -> Result<Vec<Node>> {
        let nodes: Vec<Node> = sqlx::query_as("select * from nodes where updated_at != ?")
            .bind(Node::deactivate_time())
            .fetch_all(pool)
            .await?;
        Ok(nodes)
    }

    pub async fn db_find_reforward_nodes(pool: &MySqlPool) -> Result<Vec<Node>> {
        let nodes: Vec<Node> =
            sqlx::query_as("select * from nodes where updated_at >= ? and reforward > 0")
                .bind(Node::active_time_point())
                .fetch_all(pool)
                .await?;
        Ok(nodes)
    }

    pub async fn db_find_by_addr(pool: &MySqlPool, addr: String) -> Result<Option<Node>> {
        let node: Option<Node> = sqlx::query_as("select * from nodes where addr = ?")
            .bind(addr)
            .fetch_optional(pool)
            .await?;
        Ok(node)
    }

    pub async fn max_idlest_node(pool: &MySqlPool) -> Result<Option<Node>> {
        let sql = r#"
        select * from nodes
        where
        updated_at >= ?
        and subscribe < sub_max 
        and stream < pub_max
        order by sub_max - subscribe desc limit 1
        "#;
        let node: Option<Node> = sqlx::query_as(sql)
            .bind(Node::active_time_point())
            .fetch_optional(pool)
            .await?;
        Ok(node)
    }

    pub async fn db_insert(&self, pool: &MySqlPool) -> Result<()> {
        sqlx::query(
            r#"INSERT INTO nodes ( addr, authorization, admin_authorization, pub_max, sub_max, reforward_maximum_idle_time, reforward_cascade) 
             VALUES (?, ?, ?, ?, ?, ?, ?) 
             ON DUPLICATE KEY UPDATE authorization = ?, admin_authorization =? , pub_max =?, sub_max =?,stream= ?,publish=?,subscribe=? ,reforward=? "#,
        )
        .bind(self.addr.clone())
        .bind(self.authorization.clone())
        .bind(self.admin_authorization.clone())
        .bind(self.pub_max)
        .bind(self.sub_max)
        .bind(self.reforward_maximum_idle_time)
        .bind(self.reforward_cascade)
        .bind(self.authorization.clone())
        .bind(self.admin_authorization.clone())
        .bind(self.pub_max)
        .bind(self.sub_max)
        .bind(self.stream)
        .bind(self.publish)
        .bind(self.subscribe)
        .bind(self.reforward)
        .execute(pool)
        .await?;
        Ok(())
    }

    pub async fn db_update_metrics(&self, pool: &MySqlPool) -> Result<()> {
        let rows_affected = sqlx::query(
            r#"UPDATE nodes SET stream = ?,publish = ?,subscribe=? ,reforward = ?,updated_at = ? WHERE addr = ?"#,
        )
        .bind(self.stream)
        .bind(self.publish)
        .bind(self.subscribe)
        .bind(self.reforward)
        .bind(Utc::now())
        .bind(self.addr.clone())
        .execute(pool)
        .await?
        .rows_affected() ;
        if rows_affected != 0 {
            Ok(())
        } else {
            Err(AppError::InternalServerError(anyhow::anyhow!(
                "db_update_metrics rows_affected is zero"
            )))
        }
    }

    pub async fn db_remove(&self, pool: &MySqlPool) -> Result<()> {
        sqlx::query(r#"UPDATE nodes SET updated_at = ? WHERE addr = ?"#)
            .bind(Node::deactivate_time())
            .bind(self.addr.clone())
            .execute(pool)
            .await?;
        Ok(())
    }

    pub async fn find_stream_node(pool: &MySqlPool, stream: String) -> Result<Vec<Node>> {
        let nodes: Vec<Node> = sqlx::query_as(
            r#" 
        select nodes.* from streams inner join nodes 
        on streams.addr = nodes.addr 
        where streams.stream = ?
        and nodes.updated_at >= ?
        "#,
        )
        .bind(stream)
        .bind(Node::active_time_point())
        .fetch_all(pool)
        .await?;
        Ok(nodes)
    }
}

impl Stream {
    pub async fn db_insert(&self, pool: &MySqlPool) -> Result<()> {
        sqlx::query(
            r#"INSERT INTO streams (stream,addr,publish) 
            VALUES (?, ?,1) 
            ON DUPLICATE KEY UPDATE publish=1,subscribe=0 ,reforward=0"#,
        )
        .bind(self.stream.clone().clone())
        .bind(self.addr.clone())
        .execute(pool)
        .await?;
        Ok(())
    }

    pub async fn db_remove(&self, pool: &MySqlPool) -> Result<()> {
        sqlx::query(r#"delete from streams where stream = ? and addr = ?"#)
            .bind(self.stream.clone().clone())
            .bind(self.addr.clone())
            .execute(pool)
            .await?;
        Ok(())
    }

    pub async fn db_update_metrics(&self, pool: &MySqlPool) -> Result<()> {
        sqlx::query(
            r#"UPDATE streams SET publish = ?,subscribe=? ,reforward = ? WHERE stream = ? and addr = ?"#,
        )
        .bind(self.publish)
        .bind(self.subscribe)
        .bind(self.reforward)
        .bind(self.stream.clone().clone())
        .bind(self.addr.clone())
        .execute(pool)
        .await?;
        Ok(())
    }

    pub async fn db_remove_addr_stream(pool: &MySqlPool, addr: String) -> Result<()> {
        sqlx::query(r#"delete from streams where addr = ?"#)
            .bind(addr)
            .execute(pool)
            .await?;
        Ok(())
    }

    pub async fn db_find_node_stream(pool: &MySqlPool, addr: String) -> Result<Vec<Stream>> {
        let streams: Vec<Stream> = sqlx::query_as("select * from streams where addr = ?")
            .bind(addr)
            .fetch_all(pool)
            .await?;
        Ok(streams)
    }
}
