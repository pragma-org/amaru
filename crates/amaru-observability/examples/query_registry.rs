use amaru_observability::registry::SchemaEntry;
use amaru_observability_macros::define_schemas;

// Define some schemas that should be registered
define_schemas! {
    consensus {
        chain_sync {
            VALIDATE_HEADER {
                required point_slot: u64
                required point_hash: String
                optional peer_id: String
            }
        }
    }
}

#[allow(clippy::print_stdout)]
fn main() {
    println!("Runtime Schema Registry Query Example\n");

    let count = SchemaEntry::count();
    println!("Total registered schemas: {}\n", count);

    let entries = SchemaEntry::all();
    for entry in entries {
        println!("Schema: {}", entry.path);
        println!("  Target: {}", entry.target);
        print!("  Required fields:");
        if entry.required_fields.is_empty() {
            println!(" (none)");
        } else {
            println!();
            for (name, ty) in entry.required_fields {
                println!("    - {}: {}", name, ty);
            }
        }
        print!("  Optional fields:");
        if entry.optional_fields.is_empty() {
            println!(" (none)");
        } else {
            println!();
            for (name, ty) in entry.optional_fields {
                println!("    - {}: {}", name, ty);
            }
        }
        println!();
    }

    // Query a specific schema
    if let Some(entry) = SchemaEntry::find("consensus::chain_sync::VALIDATE_HEADER") {
        println!("Found schema VALIDATE_HEADER:");
        println!("  Path: {}", entry.path);
        println!("  Required fields: {}", entry.required_fields.len());
    }
}
