;; Exercises pod-factlite end to end. Loads the binary named by FACTLITE_POD
;; and writes into FACTLITE_DIR. Prints one line per check; exits 1 on failure.
(require '[babashka.pods :as pods])

(def transport (if (= "socket" (first *command-line-args*)) :socket :stdio))
(pods/load-pod [(System/getenv "FACTLITE_POD")] {:transport transport})
(require '[pod.joshrotenberg.factlite :as f])
(println "transport:" transport)

(def failures (atom 0))
(defmacro check [label expr]
  `(let [result# (try ~expr
                      (catch Exception e#
                        (println "exception in" ~label (ex-message e#) (pr-str (ex-data e#)))
                        false))]
     (if result#
       (println "ok" ~label)
       (do (swap! failures inc) (println "FAIL" ~label)))))

(defn failure
  "The ex-data of the exception f throws, or nil."
  [f]
  (try (f) nil (catch clojure.lang.ExceptionInfo e (assoc (ex-data e) ::message (ex-message e)))))

(def path (str (System/getenv "FACTLITE_DIR") "/test-" (name transport) ".sqlite"))
(def conn (f/open path))
(check "handle" (= "factlite.store" (:handle conn)))
(check "zero head" (= 0 (:revision (f/head conn))))

(def create
  {:idempotency-key "create"
   :schema [{:name :person/name :type :string :cardinality :one :unique :none}
            {:name :person/email :type :string :cardinality :one :unique :unique-one}
            {:name :task/title :type :string :cardinality :one :unique :none}
            {:name :task/status :type :string :cardinality :one :unique :none}
            {:name :task/priority :type :i64 :cardinality :one :unique :none}
            {:name :task/owner :type :ref :cardinality :one :unique :none}
            {:name :task/tags :type :string :cardinality :many :unique :none}]
   :new-entities [:person "task"]
   :changes [[:assert {:temp :person} :person/name "Ada"]
             [:assert {:temp "task"} :task/title "Design Factlite"]
             [:assert {:temp "task"} :task/status "todo"]
             [:assert {:temp "task"} :task/priority 3]
             [:assert {:temp "task"} :task/owner {:temp "person"}]
             [:assert {:temp "task"} :task/tags "b"]
             [:assert {:temp "task"} :task/tags "a"]]
   :metadata {:source "test.bb"}})

(def a (f/transact conn create))
(def task (get-in a [:tempids :task]))
(def ada (get-in a [:tempids :person]))
(check "report" (and (= 0 (:revision (:before a)))
                     (= 1 (:revision (:after a)))
                     (= #{task ada} (set (:created-entities a)))
                     (= 7 (count (:schema-added a)))
                     (= 7 (count (:effective-delta a)))
                     (= {:source "test.bb"} (:metadata a))))
(check "pull" (= {:id (:id task) :task/status "todo" :task/owner ada :task/tags ["a" "b"]}
                 (f/pull conn task [:task/status :task/owner :task/tags])))
(check "pull unknown entity" (nil? (f/pull conn {:id (str (subs (:id task) 0 36) "00000000000000ff")} [])))

(def start {:idempotency-key "start"
            :expect [[task :task/status ["todo"]]]
            :changes [[:assert task :task/status "doing"]]})
(def b (f/transact conn start))
(check "conditional delta" (= [[:retract task :task/status "todo"] [:assert task :task/status "doing"]]
                              (:effective-delta b)))
(check "pull at A by snapshot" (= "todo" (:task/status (f/pull conn task [:task/status] {:at (:after a)}))))
(check "pull at A by revision" (= "todo" (:task/status (f/pull conn task [:task/status] {:at 1}))))
(check "retry returns original report" (= b (f/transact conn start)))
(check "idempotency conflict" (= "IdempotencyConflict"
                                 (:code (failure #(f/transact conn (assoc start :metadata {:changed true}))))))

(let [data (failure #(f/transact conn (dissoc start :idempotency-key)))]
  (check "precondition failed as data"
         (and (= "PreconditionFailed" (:code data))
              (= task (get-in data [:context :entity]))
              (= :task/status (get-in data [:context :attr]))
              (= ["todo"] (get-in data [:context :expected]))
              (= ["doing"] (get-in data [:context :actual]))
              (= "RolledBack" (:commit-outcome data))
              (string? (::message data)))))
(let [data (failure #(f/transact conn {:expected-head (:after a)}))]
  (check "head mismatch as data" (and (= "HeadMismatch" (:code data))
                                      (= (:after a) (get-in data [:context :expected]))
                                      (= (:after b) (get-in data [:context :actual])))))
(check "unique violation" (= "UniqueViolation"
                             (:code (failure #(f/transact conn {:changes [[:assert ada :person/email "a@x"]
                                                                          [:assert task :person/email "a@x"]]})))))
(check "no-change commit advances revision"
       (let [r (f/transact conn {:changes [[:assert task :task/status "doing"]]})]
         (and (= 3 (:revision (:after r))) (= [] (:effective-delta r)))))

(def query '{:inputs {:wanted :string}
             :match [[?task :task/status $wanted]
                     [?task :task/title ?title]
                     [?task :task/owner ?person]
                     [?person :person/name ?name]]
             :select [?task ?title ?name]
             :order [[?title :asc] [?task :asc]]
             :limit 20})
(check "query at head" (= {:columns [:task :title :name] :rows [[task "Design Factlite" "Ada"]]}
                          (f/q conn query {:wanted "doing"})))
(check "query at A" (= [] (:rows (f/q conn query {:wanted "doing"} {:at (:after a)}))))
(check "query predicate" (= [[3]] (:rows (f/q conn '{:match [[?t :task/priority ?p]]
                                                     :where [(< ?p 5) (not= ?p 4)]
                                                     :select [?p]}))))
(check "query missing input" (= "InvalidRequest" (:code (failure #(f/q conn query)))))

(check "history" (= [[1 :assert "todo"] [2 :retract "todo"] [2 :assert "doing"]]
                    (map (juxt :revision :op :value) (f/history conn {:entity task :attr :task/status}))))
(check "history at A" (= [[1 :assert "todo"]]
                         (map (juxt :revision :op :value)
                              (f/history conn {:entity task :attr :task/status} {:at (:after a)}))))
(check "history after" (= [2 2] (map :revision (f/history conn {:attr :task/status} {:after 1}))))
(check "transactions" (= [1 2 3] (map (comp :revision :after) (f/transactions conn))))
(check "transactions keep metadata" (= {:source "test.bb"} (:metadata (first (f/transactions conn)))))
(check "facts filter" (= [[task :task/status "doing"]] (f/facts conn {:entity task :attr :task/status})))
(check "facts value filter" (= [[task :task/tags "a"]] (f/facts conn {:attr :task/tags :value "a"})))
(check "schema at zero" (= [] (f/schema conn {:at 0})))
(check "schema" (= {:name :person/email :type :string :cardinality :one :unique :unique-one}
                   (first (filter #(= :person/email (:name %)) (f/schema conn)))))

;; Every value type maps to one Clojure type and back.
(def typed (f/transact conn {:schema [{:name :t/bool :type :bool :cardinality :many :unique :none}
                                      {:name :t/long :type :i64 :cardinality :many :unique :none}
                                      {:name :t/double :type :f64 :cardinality :many :unique :none}
                                      {:name :t/string :type :string :cardinality :many :unique :none}
                                      {:name :t/bytes :type :bytes :cardinality :many :unique :none}
                                      {:name :t/ref :type :ref :cardinality :many :unique :none}]
                             :new-entities [:x]
                             :changes [[:assert {:temp :x} :t/bool true]
                                       [:assert {:temp :x} :t/bool false]
                                       [:assert {:temp :x} :t/long Long/MAX_VALUE]
                                       [:assert {:temp :x} :t/long Long/MIN_VALUE]
                                       [:assert {:temp :x} :t/long 0]
                                       [:assert {:temp :x} :t/double 1.0]
                                       [:assert {:temp :x} :t/double -0.0]
                                       [:assert {:temp :x} :t/double 0.1]
                                       [:assert {:temp :x} :t/double 1e300]
                                       [:assert {:temp :x} :t/string "雪\u0000x"]
                                       [:assert {:temp :x} :t/string ""]
                                       [:assert {:temp :x} :t/bytes (byte-array [0 1 -1])]
                                       [:assert {:temp :x} :t/ref task]
                                       [:assert {:temp :x} :t/ref {:temp :x}]]}))
(def x (get-in typed [:tempids :x]))
(def pulled (f/pull conn x [:t/bool :t/long :t/double :t/string :t/bytes :t/ref]))
(check "bool" (= [false true] (:t/bool pulled)))
(check "long range" (= [Long/MIN_VALUE 0 Long/MAX_VALUE] (:t/long pulled)))
(check "longs stay longs" (every? #(instance? Long %) (:t/long pulled)))
(check "doubles stay doubles" (and (= [0.0 0.1 1.0 1e300] (:t/double pulled))
                                   (every? #(instance? Double %) (:t/double pulled))))
(check "negative zero normalized" (= 0 (Double/compare 0.0 (first (:t/double pulled)))))
(check "strings exact" (= ["" "雪\u0000x"] (:t/string pulled)))
(check "bytes" (= [[0 1 -1]] (map vec (:t/bytes pulled))))
(check "refs" (= #{task x} (set (:t/ref pulled))))
(check "double filter matches double" (= [[x :t/double 1.0]] (f/facts conn {:attr :t/double :value 1.0})))
(check "type mismatch" (= "TypeMismatch" (:code (failure #(f/facts conn {:attr :t/double :value 1})))))

(f/transact conn {:schema [{:name :t/kw :type :keyword :cardinality :many :unique :none}
                           {:name :t/at :type :inst :cardinality :many :unique :none}]
                  :changes [[:assert x :t/kw :status/done] [:assert x :t/kw :todo]
                            [:assert x :t/at #inst "2026-11-05T12:00:00.000-00:00"]
                            [:assert x :t/at #inst "1969-12-31T23:59:59.999-00:00"]]})
(def typed-new (f/pull conn x [:t/kw :t/at]))
(check "keywords round trip" (= [:status/done :todo] (:t/kw typed-new)))
(check "insts round trip" (= [#inst "1969-12-31T23:59:59.999-00:00" #inst "2026-11-05T12:00:00.000-00:00"] (:t/at typed-new)))
(check "insts are dates" (every? #(instance? java.util.Date %) (:t/at typed-new)))
(check "keyword constant in query" (= [[x]] (:rows (f/q conn '{:match [[?e :t/kw :status/done]] :select [?e]}))))
(check "inst range with an input"
       (= [[#inst "2026-11-05T12:00:00.000-00:00"]]
          (:rows (f/q conn '{:inputs {:since :inst} :match [[?e :t/at ?at]] :where [(>= ?at $since)] :select [?at]}
                      {:since #inst "2000-01-01T00:00:00.000-00:00"}))))
(check "inst min and max" (= [[#inst "1969-12-31T23:59:59.999-00:00" #inst "2026-11-05T12:00:00.000-00:00"]]
                             (:rows (f/q conn '{:match [[?e :t/at ?at]] :select [(min ?at) (max ?at)]}))))
(check "keyword is not a string" (= "TypeMismatch" (:code (failure #(f/transact conn {:changes [[:assert task :task/status :done]]})))))
(check "inst is not a long" (= "TypeMismatch" (:code (failure #(f/transact conn {:changes [[:assert task :task/priority #inst "2026-01-01"]]})))))
(check "invalid keyword text" (= "InvalidRequest" (:code (failure #(f/transact conn {:changes [[:assert x :t/kw (keyword "a b")]]})))))
(check "keyword schema type" (= :keyword (:type (first (filter #(= :t/kw (:name %)) (f/schema conn))))))

(doseq [[label value] [["nil" nil] ["BigInt" 1N] ["ratio" 1/3] ["BigDecimal" 1.5M]
                       ["NaN" Double/NaN] ["infinity" ##Inf] ["vector" ["a"]] ["set" #{"a"}]
                       ["char" \a] ["uuid" (random-uuid)]]]
  (check (str "rejects " label)
         (let [data (failure #(f/transact conn {:changes [[:assert task :task/status value]]}))]
           (and (= "InvalidRequest" (:code data)) (some? (:context data))))))
(check "unknown transaction key" (= "InvalidRequest" (:code (failure #(f/transact conn {:chnges []})))))
(check "string keys rejected" (= "InvalidRequest" (:code (failure #(f/transact conn {"changes" []})))))
(check "unknown attribute" (= "UnknownAttribute"
                              (:code (failure #(f/pull conn task [:task/nope])))))
(check "future revision" (= "InvalidSnapshot" (:code (failure #(f/pull conn task [] {:at 99})))))
(check "foreign snapshot" (= "InvalidSnapshot"
                             (:code (failure #(f/schema conn {:at (assoc (:after a) :store "s1.00000000000000000000000000000001")})))))
(check "bad arity" (= "InvalidRequest" (:code (failure #(f/head conn conn)))))
(check "unknown opts key" (= "InvalidRequest" (:code (failure #(f/schema conn {:as-of 1})))))

(def emailed (f/transact conn {:changes [[:assert ada :person/email "ada@example.com"]]}))
(check "lookup" (= ada (f/lookup conn [:person/email "ada@example.com"])))
(check "lookup before assignment" (nil? (f/lookup conn [:person/email "ada@example.com"] {:at (:after a)})))
(check "lookup missing value" (nil? (f/lookup conn [:person/email "nobody@example.com"])))
(check "pull by lookup ref" (= {:id (:id ada) :person/name "Ada"}
                               (f/pull conn [:person/email "ada@example.com"] [:person/name])))
(check "pull by unmatched lookup ref" (nil? (f/pull conn [:person/email "nobody@example.com"] [:person/name])))
(check "unmatched lookup still validates attrs"
       (= "UnknownAttribute" (:code (failure #(f/pull conn [:person/email "nobody@example.com"] [:person/nope])))))
(check "facts by lookup ref" (= [[ada :person/name "Ada"]]
                                (f/facts conn {:entity [:person/email "ada@example.com"] :attr :person/name})))
(check "facts by unmatched lookup ref" (= [] (f/facts conn {:entity [:person/email "nobody@example.com"]})))
(check "history by lookup ref" (= [(:revision (:after emailed))]
                                  (map :revision (f/history conn {:entity [:person/email "ada@example.com"]
                                                                  :attr :person/email}))))
(check "lookup on non-unique attribute" (= "InvalidRequest" (:code (failure #(f/lookup conn [:person/name "Ada"])))))
(check "lookup with wrong type" (= "TypeMismatch" (:code (failure #(f/lookup conn [:person/email 1])))))
(check "lookup refs are not transaction entities"
       (= "InvalidRequest" (:code (failure #(f/transact conn {:changes [[:assert [:person/email "ada@example.com"] :person/name "A"]]})))))

(f/transact conn {:schema [{:name :agg/recipe :type :string :cardinality :one :unique :none}
                           {:name :agg/cost :type :f64 :cardinality :one :unique :none}]
                  :new-entities [:r1 :r2 :r3 :r4]
                  :changes [[:assert {:temp :r1} :agg/recipe "a"] [:assert {:temp :r1} :agg/cost 1.5]
                            [:assert {:temp :r2} :agg/recipe "a"] [:assert {:temp :r2} :agg/cost 1.5]
                            [:assert {:temp :r3} :agg/recipe "a"] [:assert {:temp :r3} :agg/cost 2.0]
                            [:assert {:temp :r4} :agg/recipe "b"] [:assert {:temp :r4} :agg/cost 4.0]]})
(def per-recipe '{:match [[?run :agg/recipe ?recipe] [?run :agg/cost ?cost]]
                  :select [?recipe (sum ?cost) (count ?run)]
                  :with [?run]
                  :order [[(sum ?cost) :desc]]})
(check "aggregate columns in select order"
       (= [:recipe :sum-cost :count-run] (:columns (f/q conn per-recipe))))
(check "sum and count per group with :with"
       (= [["a" 5.0 3] ["b" 4.0 1]] (:rows (f/q conn per-recipe))))
(check "without :with equal tuples collapse"
       (= #{["a" 3.5] ["b" 4.0]}
          (set (:rows (f/q conn '{:match [[?run :agg/recipe ?recipe] [?run :agg/cost ?cost]]
                                  :select [?recipe (sum ?cost)]})))))
(check "global aggregates"
       (= [[4 1.5 4.0 2]] (:rows (f/q conn '{:match [[?run :agg/cost ?cost] [?run :agg/recipe ?recipe]]
                                             :select [(count ?run) (min ?cost) (max ?cost) (count-distinct ?recipe)]}))))
(check "aggregate with no matches has no rows"
       (= [] (:rows (f/q conn '{:match [[?run :agg/cost ?cost] [?run :task/status ?s]]
                                :select [(count ?run)]}))))
(check "sum of a string fails" (= "TypeMismatch" (:code (failure #(f/q conn '{:match [[?r :agg/recipe ?x]] :select [(sum ?x)]})))))
(check "unknown aggregate fails" (= "InvalidRequest" (:code (failure #(f/q conn '{:match [[?r :agg/cost ?x]] :select [(avg ?x)]})))))

(def reader (f/open path {:read-only true :busy-timeout-ms 500}))
(check "read-only head follows writer" (= (f/head conn) (f/head reader)))
(check "read-only history" (= "todo" (:task/status (f/pull reader task [:task/status] {:at (:after a)}))))
(check "read-only refuses commits" (= "InvalidRequest" (:code (failure #(f/transact reader {})))))
(let [later (f/transact conn {:metadata {:after-reader true}})]
  (check "read-only sees later commits" (= (:after later) (f/head reader))))
(f/close reader)
(let [instant (f/open path {:busy-timeout-ms 0})]
  (check "busy timeout zero" (= (f/head conn) (f/head instant)))
  (f/close instant))
(check "rejects negative timeout" (= "InvalidRequest" (:code (failure #(f/open path {:busy-timeout-ms -1})))))
(check "rejects unknown open option" (= "InvalidRequest" (:code (failure #(f/open path {:readonly true})))))
(check "read-only open of missing file fails" (= "Storage" (:code (failure #(f/open (str path ".missing") {:read-only true})))))

(f/close conn)
(check "closed connection" (re-find #"closed" (::message (failure #(f/head conn)))))
(def reopened (f/open path))
(check "reopen keeps history" (= "todo" (:task/status (f/pull reopened task [:task/status] {:at (:after a)}))))
(check "reopen retry" (= b (f/transact reopened start)))
(check "doc" (some? (:doc (meta #'f/transact))))

(println "failures:" @failures)
(println "done")
(System/exit (if (zero? @failures) 0 1))
