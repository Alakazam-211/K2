-- Monthly revenue per region, newest first.
WITH monthly AS (
    SELECT
        region,
        date_trunc('month', placed_at) AS month,
        SUM(amount) AS revenue
    FROM orders
    WHERE status <> 'cancelled'
    GROUP BY 1, 2
)
SELECT region, month, revenue,
       revenue - LAG(revenue) OVER (PARTITION BY region ORDER BY month) AS delta
FROM monthly
ORDER BY month DESC, region;
