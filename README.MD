# Auction

Exonum test

## Functionality
### Create auction - `POST`
```
http://host:8000/api/services/auction/create
```
Parameters:

* `pub_key` - Public key of the lot's owner
* `descr` - Text of description
* `start_price` - Start price

### Get lot details - `GET` 
```
http://host:8000/api/services/auction/lot?lot_id={LOT_ID}

```
Parameters:
* `LOT_ID` - The transaction hash of which the lot was created
 
### Make bid - `POST`
```
http://host:8000/api/services/auction/bid
```

Parameters:

* `pub_key` - Public key of the lot's owner
* `lot_id` - The transaction hash of which the lot was created
* `value` - The bid value


