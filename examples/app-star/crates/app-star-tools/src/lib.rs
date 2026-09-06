// VISION FIXTURE — target state, not yet buildable (E-1 lane; attribute
// shape illustrative). The muscle surface: name, schema, description, and
// capabilities derive from the signature + attribute — one declaration per
// fact. If [allow] does not grant the claimed host, `tau check` FAILS AT
// BUILD: an un-grantable tool cannot exist (worked-examples X1).

use tau::tool::ToolError;

#[tau::tool(caps(net = "billing.corp.internal:443"))]
/// Look up a customer's billing standing.
fn billing_lookup(customer_id: CustomerId) -> Result<BillingStanding, ToolError> {
    billing_client::standing(&customer_id)
}

tau::export![billing_lookup];
