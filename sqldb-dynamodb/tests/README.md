# sqldb-dynamodb capability provider tests

The tests include a local docker dynamodb database (`docker/dynamodb/shared-local-instance.db`). The database was initialized with the following command from example 6 of the [AWS cli guide](https://docs.aws.amazon.com/cli/latest/reference/dynamodb/create-table.html)
```
aws dynamodb create-table \
    --table-name GameScores \
    --attribute-definitions AttributeName=UserId,AttributeType=S AttributeName=GameTitle,AttributeType=S AttributeName=TopScore,AttributeType=N AttributeName=Date,AttributeType=S \
    --key-schema AttributeName=UserId,KeyType=HASH AttributeName=GameTitle,KeyType=RANGE \
    --provisioned-throughput ReadCapacityUnits=10,WriteCapacityUnits=5 \
    --global-secondary-indexes file://gsi.json
```

The file gsi.json is included in case the database needs to be recreated.
