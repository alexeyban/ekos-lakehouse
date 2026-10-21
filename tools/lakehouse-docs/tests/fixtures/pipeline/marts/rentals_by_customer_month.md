---
mart: rentals_by_customer_month
grain: [customer, month]
dimensions:
  customer: customer.customer_id
  customer_email: customer.email
  month: month of rental.rental_date
measures:
  rentals: count of rental.rental_id
  revenue: sum of payment.amount
filters:
  - rental.return_date is not null
---
How many films each customer rented per month, and what they paid for them.
Used by the retention team to spot customers whose activity is falling.
