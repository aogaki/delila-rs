// Initialize DELILA database with collections and indexes

db = db.getSiblingDB('delila');

// Create runs collection with schema validation
db.createCollection('runs', {
  validator: {
    $jsonSchema: {
      bsonType: 'object',
      required: ['run_number', 'exp_name', 'start_time', 'status'],
      properties: {
        run_number: {
          bsonType: 'int',
          description: 'Run number - required'
        },
        exp_name: {
          bsonType: 'string',
          description: 'Experiment name - required'
        },
        comment: {
          bsonType: 'string',
          description: 'Optional comment'
        },
        start_time: {
          bsonType: 'date',
          description: 'Run start time - required'
        },
        end_time: {
          bsonType: ['date', 'null'],
          description: 'Run end time - null if still running'
        },
        duration_secs: {
          bsonType: ['int', 'null'],
          description: 'Run duration in seconds'
        },
        status: {
          enum: ['running', 'completed', 'error', 'aborted'],
          description: 'Run status - required'
        },
        stats: {
          bsonType: 'object',
          properties: {
            total_events: { bsonType: 'long' },
            total_bytes: { bsonType: 'long' },
            average_rate: { bsonType: 'double' }
          }
        },
        config_snapshot: {
          bsonType: 'object',
          description: 'Configuration used for this run'
        },
        errors: {
          bsonType: 'array',
          items: {
            bsonType: 'object',
            properties: {
              time: { bsonType: 'date' },
              component: { bsonType: 'string' },
              message: { bsonType: 'string' }
            }
          }
        }
      }
    }
  }
});

// Create indexes
// Non-unique index for querying by exp_name and run_number
// (same run_number can be reused for retakes, creating multiple documents)
db.runs.createIndex({ exp_name: 1, run_number: 1 });
db.runs.createIndex({ exp_name: 1, start_time: -1 });  // For next_run_number query
db.runs.createIndex({ status: 1 });

print('DELILA database initialized successfully');
