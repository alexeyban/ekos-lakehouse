import json
def col(n,t,nn=False,attnum=0,comment=None,kind="Base",enum=None,pii=None,trino=None,default=False):
    c={"name":n,"pg_type":t,"kind":{"kind":kind.lower()},"not_null":nn,"trino_type":trino or {"integer":"integer","smallint":"smallint","text":"varchar","character varying(45)":"varchar(45)","character varying(50)":"varchar(50)","character varying(20)":"varchar(20)","timestamp with time zone":"timestamp(6) with time zone","timestamp without time zone":"timestamp(6)","numeric(5,2)":"decimal(5,2)","numeric(4,2)":"decimal(4,2)","boolean":"boolean","date":"date","character(20)":"char(20)"}.get(t,"varchar"),
       "numeric_used":None,"attnum":attnum,"has_default":default}
    if comment: c["comment"]=comment
    if enum: c["enum_labels"]=enum
    if pii: c["pii_candidate"]=pii
    return c
def tbl(name,oid,cols,pk,rows,fks=(),comment=None,trig=True):
    for i,c in enumerate(cols,1): c["attnum"]=i
    t={"schema":"public","name":name,"oid":oid,"columns":cols,"primary_key":pk,"row_estimate":rows,
       "update_triggers":[{"trigger":"last_updated","function":"last_updated","assigns":["last_update"],"body_sha":"ab12"}] if trig else [],
       "foreign_keys":[{"name":f[0],"columns":f[1],"ref_schema":"public","ref_table":f[2],"ref_columns":f[3]} for f in fks]}
    if comment: t["comment"]=comment
    return t
lu=lambda: col("last_update","timestamp with time zone",True,default=True)
T=[
 tbl("language",16400,[col("language_id","integer",True,default=True),col("name","character(20)",True),lu()],["language_id"],6,comment="Languages films are available in"),
 tbl("film",16410,[col("film_id","integer",True,default=True),col("title","text",True,comment="Film title as shown in the catalogue"),col("description","text"),col("release_year","integer"),col("language_id","integer",True),col("rental_duration","smallint",True),col("rental_rate","numeric(4,2)",True,comment="Price for one rental period"),col("length","smallint"),col("rating","mpaa_rating",kind="Enum",enum=["G","PG","PG-13","R","NC-17"],trino="varchar"),lu()],["film_id"],1000,[("film_language_id_fkey",["language_id"],"language",["language_id"])],"Every film in the rental catalogue"),
 tbl("city",16420,[col("city_id","integer",True),col("city","text",True),col("country_id","integer",True),lu()],["city_id"],600),
 tbl("address",16430,[col("address_id","integer",True),col("address","text",True,pii="address"),col("district","text",True),col("city_id","integer",True),col("postal_code","text"),col("phone","text",True,pii="phone"),lu()],["address_id"],603,[("address_city_id_fkey",["city_id"],"city",["city_id"])]),
 tbl("customer",16440,[col("customer_id","integer",True),col("store_id","integer",True),col("first_name","text",True,pii="person_name"),col("last_name","text",True,pii="person_name"),col("email","text",pii="email"),col("address_id","integer",True),col("activebool","boolean",True),col("create_date","date",True),lu(),col("active","integer")],["customer_id"],599,[("customer_address_id_fkey",["address_id"],"address",["address_id"])],"Customers who have rented at least once"),
 tbl("inventory",16450,[col("inventory_id","integer",True),col("film_id","integer",True),col("store_id","integer",True),lu()],["inventory_id"],4581,[("inventory_film_id_fkey",["film_id"],"film",["film_id"])]),
 tbl("rental",16460,[col("rental_id","integer",True),col("rental_date","timestamp with time zone",True),col("inventory_id","integer",True),col("customer_id","integer",True),col("return_date","timestamp with time zone"),col("staff_id","integer",True),lu()],["rental_id"],16044,[("rental_inventory_id_fkey",["inventory_id"],"inventory",["inventory_id"]),("rental_customer_id_fkey",["customer_id"],"customer",["customer_id"])],"One row per rental of one inventory item"),
 tbl("payment",16470,[col("payment_id","integer",True),col("customer_id","integer",True),col("staff_id","integer",True),col("rental_id","integer",True),col("amount","numeric(5,2)",True,comment="Amount paid, in USD"),col("payment_date","timestamp with time zone",True)],["payment_id"],16049,[("payment_customer_id_fkey",["customer_id"],"customer",["customer_id"]),("payment_rental_id_fkey",["rental_id"],"rental",["rental_id"])],"Payments against rentals",trig=False),
]
json.dump(T,open(__import__("pathlib").Path(__file__).with_name("sources.json"),"w"),indent=1)
