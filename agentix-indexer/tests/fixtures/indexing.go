package fixture

// Identity keeps a Unicode λ marker.
func Identity(n int) int { return n }

type Point struct { X int }
type Reader interface { Read() int }
type Alias = int

func (p *Point) Read() int { return p.X }

const (First = 1; Second = 2)
var Global = 3

// func Fake() {}
/* type AlsoFake struct {} */
