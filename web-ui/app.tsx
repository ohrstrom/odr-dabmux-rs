import { createRoot } from "react-dom/client"
import { Button } from "@/components/ui/button"

function App() {
  return (
    <main className="p-8">
      <h1 className="text-3xl font-bold">ODR-DabMux</h1>

      <p className="mt-4 text-gray-500">
        Hello from React + Bun + Tailwind.
      </p>
      <Button className="mt-4">Click Me</Button>
    </main>
  )
}

createRoot(document.getElementById("root")!).render(<App />)