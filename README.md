# Drama

A dramatically minimal actor library. Check out the test(s) for websocket usage examples.

```rust
#[derive(Debug)]
struct Testor {
  foos: usize,
  bars: isize,
}

#[derive(Debug, Clone)]
struct Foo {}

#[derive(Debug, Clone)]
struct Bar {}

impl Actor for Testor {}

#[async_trait]
impl Handler<Foo> for Testor {
  type Response = usize;
  async fn handle(&mut self, _: &Foo) -> usize {
      self.foos += 1;
      10
  }
}
    

#[async_trait]
impl Handler<Bar> for Testor {
  type Response = isize;
  async fn handle(&mut self, _: &Bar) -> isize {
      self.bars += 1;
      if self.foos == 100 {
        assert_eq!(self.bars, 100);
      }
      10
  }
}

let mut res = 0;
let mut res2 = 0;

let handle = Testor { foos: 0, bars: 0 }.start();

for _ in 0..100 {
  res += handle.send_wait(Foo {}).await.unwrap();
  res2 += handle.send_wait(Bar {}).await.unwrap();
}

handle.send(Foo {}).unwrap();
handle.send_forget(Bar {});

let rec: Recipient<Foo> = handle.recipient();
rec.send(Foo {}).unwrap();
handle.send_cmd(ActorCommand::Stop).unwrap();

assert_eq!(res, 1000);
assert_eq!(res2, 1000);
```
