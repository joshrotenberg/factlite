;; One of several concurrent writers. Args: transport, worker id, iterations,
;; counter entity id. Each iteration commits one keyed new entity and then one
;; conditional counter increment, rereading and retrying only on
;; PreconditionFailed or Busy. Prints its retry count, then "done".
(require '[babashka.pods :as pods])

(let [[transport worker iterations counter] *command-line-args*]
  (pods/load-pod [(System/getenv "FACTLITE_POD")] {:transport (keyword transport)})
  (require '[pod.joshrotenberg.factlite :as f])
  (let [open (resolve 'pod.joshrotenberg.factlite/open)
        pull (resolve 'pod.joshrotenberg.factlite/pull)
        transact (resolve 'pod.joshrotenberg.factlite/transact)
        conn (open (System/getenv "FACTLITE_DB"))
        counter {:id counter}
        retries (atom 0)
        retryable? (fn [e] (contains? #{"PreconditionFailed" "Busy"} (:code (ex-data e))))]
    (dotimes [n (parse-long iterations)]
      (loop []
        (let [result (try
                       (transact conn {:idempotency-key (str "w" worker "-" n)
                                       :new-entities [:run]
                                       :changes [[:assert {:temp :run} :run/worker (parse-long worker)]
                                                 [:assert {:temp :run} :run/n n]]})
                       (catch clojure.lang.ExceptionInfo e
                         (if (= "Busy" (:code (ex-data e))) ::retry (throw e))))]
          (when (= ::retry result) (swap! retries inc) (recur))))
      (loop []
        (let [current (:count/value (pull conn counter [:count/value]))
              result (try
                       (transact conn {:expect [[counter :count/value [current]]]
                                       :changes [[:assert counter :count/value (inc current)]]})
                       (catch clojure.lang.ExceptionInfo e
                         (if (retryable? e) ::retry (throw e))))]
          (when (= ::retry result) (swap! retries inc) (recur)))))
    (println "retries:" @retries)
    (println "done")))
